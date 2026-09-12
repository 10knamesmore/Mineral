//! Shared download services, session control, and observable lifecycle state.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mineral_channel_core::MusicChannel;
use mineral_model::BitRate;
use mineral_playback::PlaybackRegistry;
use mineral_protocol::{DownloadId, DownloadStatus, DownloadSummary, SongDownloadView};
use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::state::ManagerState;

/// Immutable services used by download attempts and playlist expansion.
#[derive(Clone)]
pub(crate) struct DownloadRuntime {
    /// Permanent export root; `None` makes Song attempts fail as unavailable.
    pub(crate) music_dir: Option<PathBuf>,

    /// Catalog channels used only to expand playlist snapshots.
    pub(crate) channels: Vec<Arc<dyn MusicChannel>>,

    /// Source-neutral media providers used by Song attempts.
    pub(crate) playback: PlaybackRegistry,

    /// Script interception gate used before opening download media.
    pub(crate) hooks: crate::hook_bridge::HookGate,

    /// Post-commit metadata tagging queue.
    pub(crate) tagging: crate::tagging::TaggingQueue,

    /// Wire and script lifecycle notifications.
    pub(crate) notify: crate::notify::Notifier,

    /// Download behavior recorder.
    pub(crate) stats: crate::StatsRecorder,

    /// Transfer-speed sampling interval.
    pub(crate) speed_tick: Duration,
}

/// Cloneable handle to the only Song download lifecycle owner.
#[derive(Clone)]
pub(crate) struct DownloadManager {
    /// Shared manager state and scheduler signals.
    pub(super) inner: Arc<ManagerInner>,
}

/// Shared manager implementation.
pub(super) struct ManagerInner {
    /// Immutable execution services.
    pub(super) runtime: DownloadRuntime,

    /// Mutable lifecycle state.
    pub(super) state: Mutex<ManagerState>,

    /// Wakes the admission loop after submit, completion, Stop, or config change.
    pub(super) wake: Notify,

    /// 明细变更 generation:订阅者被唤醒后拉取快照 / 增量。
    pub(super) changes: tokio::sync::watch::Sender<u64>,

    /// 下一个变更 generation。
    pub(super) change_seq: AtomicU64,

    /// Wakes graceful shutdown waiters after active-attempt changes.
    pub(super) quiesced: Notify,

    /// Stops new admission and the scheduler loop.
    pub(super) shutdown: CancellationToken,
}

impl DownloadManager {
    /// Creates the manager and starts its admission loop.
    ///
    /// # Params:
    ///   - `runtime`: Immutable execution services.
    ///   - `quality`: Quality applied to new submissions.
    ///   - `max_concurrent`: Maximum active Song attempts, validated as positive by config loading.
    pub(crate) fn spawn(runtime: DownloadRuntime, quality: BitRate, max_concurrent: usize) -> Self {
        let (changes, _changes_rx) = tokio::sync::watch::channel(0_u64);
        let manager = Self {
            inner: Arc::new(ManagerInner {
                runtime,
                state: Mutex::new(ManagerState::new(quality, max_concurrent)),
                wake: Notify::new(),
                changes,
                change_seq: AtomicU64::new(0),
                quiesced: Notify::new(),
                shutdown: CancellationToken::new(),
            }),
        };
        if let Some(root) = manager.inner.runtime.music_dir.as_deref() {
            crate::download::cleanup_orphan_partials(root);
        }
        let scheduler = manager.clone();
        tokio::spawn(async move { scheduler.run_scheduler().await });
        manager
    }

    /// Applies download config to future admission without cancelling active attempts.
    ///
    /// # Params:
    ///   - `quality`: Quality for later submissions.
    ///   - `max_concurrent`: New active cap, validated as positive by config loading.
    pub(crate) fn set_config(&self, quality: BitRate, max_concurrent: usize) {
        let mut state = self.inner.state.lock();
        state.quality = quality;
        state.max_concurrent = max_concurrent;
        drop(state);
        self.inner.wake.notify_one();
        self.bump();
    }

    /// 明细变更订阅端(初值为当前 generation)。
    pub(crate) fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.changes.subscribe()
    }

    /// 发布一次明细变更(无订阅者时静默)。
    pub(super) fn bump(&self) {
        let next = self
            .inner
            .change_seq
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let _ = self.inner.changes.send(next);
    }

    /// Returns the small per-tick summary.
    pub(crate) fn summary(&self) -> DownloadSummary {
        self.inner.state.lock().summary()
    }

    /// Returns active, queued, then newest-first terminal rows.
    pub(crate) fn snapshot(&self) -> Vec<SongDownloadView> {
        self.inner.state.lock().snapshot()
    }

    /// Stops a known download; terminal rows are unchanged and unknown IDs return an error.
    pub(crate) fn stop(&self, id: &DownloadId) -> color_eyre::Result<()> {
        let mut cancellation = None::<CancellationToken>;
        let mut queued_stop = false;
        let song_id = {
            let mut state = self.inner.state.lock();
            let row = state
                .rows
                .get(id)
                .ok_or_else(|| color_eyre::eyre::eyre!("unknown download identity"))?;
            let status = row.view.status;
            let song_id = row.view.song.id.clone();
            match status {
                DownloadStatus::Queued => {
                    queued_stop = true;
                    state.mark_terminal(id, DownloadStatus::Stopped, None);
                }
                DownloadStatus::Resolving
                | DownloadStatus::Downloading
                | DownloadStatus::Finalizing => {
                    if let Some(row) = state.rows.get_mut(id) {
                        row.view.status = DownloadStatus::Stopping;
                        row.view.speed_bps = 0;
                    }
                    cancellation = state.active.get(id).cloned();
                }
                DownloadStatus::Stopping
                | DownloadStatus::Stopped
                | DownloadStatus::Downloaded
                | DownloadStatus::AlreadyPresent
                | DownloadStatus::SkippedByHook
                | DownloadStatus::Failed => {}
            }
            state.finish_wave_if_idle();
            song_id
        };
        if let Some(token) = cancellation {
            token.cancel();
        }
        mineral_log::info!(
            target: "download",
            download_id = %id,
            song_id = %song_id.qualified(),
            source = song_id.namespace().name(),
            queued = queued_stop,
            "download Stop accepted"
        );
        self.inner.wake.notify_one();
        self.bump();
        Ok(())
    }

    /// Cancels all work and waits for active writers to quiesce.
    pub(crate) async fn shutdown(&self) {
        self.inner.shutdown.cancel();
        let tokens = {
            let mut state = self.inner.state.lock();
            let queued = state
                .rows
                .iter()
                .filter(|(_, row)| row.view.status == DownloadStatus::Queued)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            for id in queued {
                state.mark_terminal(&id, DownloadStatus::Stopped, None);
            }
            let active_ids = state.active.keys().cloned().collect::<Vec<_>>();
            for id in active_ids {
                if let Some(row) = state.rows.get_mut(&id) {
                    row.view.status = DownloadStatus::Stopping;
                    row.view.speed_bps = 0;
                }
            }
            state.active.values().cloned().collect::<Vec<_>>()
        };
        for token in tokens {
            token.cancel();
        }
        loop {
            let quiesced = self.inner.quiesced.notified();
            if self.inner.state.lock().active.is_empty() {
                break;
            }
            quiesced.await;
        }
    }
}
