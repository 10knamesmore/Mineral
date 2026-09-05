//! Session-only owner for flat Song download lifecycles.

mod completion;
mod state;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mineral_channel_core::MusicChannel;
use mineral_model::{BitRate, Song};
use mineral_playback::PlaybackRegistry;
use mineral_protocol::{
    DownloadId, DownloadOrigin, DownloadStatus, DownloadSummary, DownloadTarget, PlaylistRef,
    SongDownloadView, ToastKind,
};
use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use self::completion::{CompletionEffect, record_download};
use self::state::{DownloadKey, ManagerState, SongDownload, pop_queued};
use super::{
    DownloadAttempt, DownloadEnv, DownloadOutcome, SkipCause, TransferUpdate, download_song,
};

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
    inner: Arc<ManagerInner>,
}

/// Shared manager implementation.
struct ManagerInner {
    /// Immutable execution services.
    runtime: DownloadRuntime,

    /// Mutable lifecycle state.
    state: Mutex<ManagerState>,

    /// Wakes the admission loop after submit, completion, Stop, or config change.
    wake: Notify,

    /// Wakes graceful shutdown waiters after active-attempt changes.
    quiesced: Notify,

    /// Stops new admission and the scheduler loop.
    shutdown: CancellationToken,
}

/// Scheduler lane of a newly admitted row.
#[derive(Clone, Copy)]
enum Lane {
    /// Direct Song submission.
    Direct,

    /// Song expanded from a playlist.
    Playlist,
}

/// One execution detached from the state lock; each DownloadId executes at most once.
struct AdmittedAttempt {
    /// Download identity.
    id: DownloadId,

    /// Song snapshot.
    song: Song,

    /// Requested quality.
    quality: BitRate,

    /// Cooperative cancellation handle.
    cancellation: CancellationToken,
}

impl DownloadManager {
    /// Creates the manager and starts its admission loop.
    ///
    /// # Params:
    ///   - `runtime`: Immutable execution services.
    ///   - `quality`: Quality applied to new submissions.
    ///   - `max_concurrent`: Maximum active Song attempts, validated as positive by config loading.
    pub(crate) fn spawn(runtime: DownloadRuntime, quality: BitRate, max_concurrent: usize) -> Self {
        let manager = Self {
            inner: Arc::new(ManagerInner {
                runtime,
                state: Mutex::new(ManagerState::new(quality, max_concurrent)),
                wake: Notify::new(),
                quiesced: Notify::new(),
                shutdown: CancellationToken::new(),
            }),
        };
        if let Some(root) = manager.inner.runtime.music_dir.as_deref() {
            super::cleanup_orphan_partials(root);
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
    }

    /// Accepts a Song immediately or starts asynchronous playlist expansion.
    pub(crate) fn submit(&self, target: DownloadTarget) {
        match target {
            DownloadTarget::Song(song) => {
                self.admit_song(&song, DownloadOrigin::Direct, Lane::Direct);
            }
            DownloadTarget::Playlist(id) => {
                {
                    let mut state = self.inner.state.lock();
                    state.preparing_playlists = state.preparing_playlists.saturating_add(1);
                }
                let manager = self.clone();
                tokio::spawn(async move { manager.expand_playlist(id).await });
            }
        }
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

    /// Expands one canonical playlist snapshot into flat Song admissions.
    async fn expand_playlist(&self, id: mineral_model::PlaylistId) {
        let channel = self
            .inner
            .runtime
            .channels
            .iter()
            .find(|channel| channel.source() == id.namespace())
            .cloned();
        let result = match channel {
            Some(channel) => channel.playlist_detail(&id).await.map_err(|error| {
                mineral_log::warn!(target: "download", playlist_id = %id.qualified(), error = mineral_log::chain(&error), "playlist expansion failed");
                "Download failed: could not load playlist".to_owned()
            }),
            None => Err("Download failed: source has no catalog channel".to_owned()),
        };
        match result {
            Ok(playlist) if !self.inner.shutdown.is_cancelled() => {
                let origin = DownloadOrigin::Playlist(PlaylistRef {
                    id: playlist.id,
                    name: playlist.name,
                });
                for entry in playlist.entries {
                    self.admit_song(&entry.song, origin.clone(), Lane::Playlist);
                }
            }
            Ok(_) => {}
            Err(message) if !self.inner.shutdown.is_cancelled() => {
                self.inner.runtime.notify.toast(ToastKind::Warn, message);
            }
            Err(_) => {}
        }
        {
            let mut state = self.inner.state.lock();
            state.preparing_playlists = state.preparing_playlists.saturating_sub(1);
            state.finish_wave_if_idle();
        }
        self.inner.wake.notify_one();
    }

    /// Inserts or deduplicates one flat Song row.
    fn admit_song(&self, song: &Song, origin: DownloadOrigin, lane: Lane) {
        let mut state = self.inner.state.lock();
        let key = DownloadKey {
            export_root: self.inner.runtime.music_dir.clone(),
            song_id: song.id.clone(),
            quality: state.quality,
        };
        if let Some(existing) = state.dedup.get(&key) {
            mineral_log::info!(
                target: "download",
                download_id = %existing,
                song_id = %song.id.qualified(),
                source = song.source().name(),
                "download submission deduplicated"
            );
            return;
        }
        let id = DownloadId::new(uuid::Uuid::new_v4().to_string());
        let admission_order = state.next_admission;
        state.next_admission = state.next_admission.wrapping_add(1);
        let quality = state.quality;
        let row = SongDownload {
            view: SongDownloadView {
                id: id.clone(),
                song: Box::new(song.clone()),
                origin,
                status: DownloadStatus::Queued,
                quality,
                bytes_done: 0,
                bytes_total: None,
                speed_bps: 0,
                failure: None,
            },
            key: key.clone(),
            admission_order,
        };
        state.rows.insert(id.clone(), row);
        state.dedup.insert(key, id.clone());
        match lane {
            Lane::Direct => state.direct_queue.push_back(id.clone()),
            Lane::Playlist => state.playlist_queue.push_back(id.clone()),
        }
        state.open_wave();
        drop(state);
        mineral_log::info!(
            target: "download",
            download_id = %id,
            song_id = %song.id.qualified(),
            source = song.source().name(),
            "Song download admitted"
        );
        self.inner.wake.notify_one();
    }

    /// Runs the bounded admission loop until graceful shutdown.
    async fn run_scheduler(&self) {
        loop {
            tokio::select! {
                () = self.inner.shutdown.cancelled() => break,
                () = self.inner.wake.notified() => {
                    while let Some(attempt) = self.take_next() {
                        let manager = self.clone();
                        tokio::spawn(async move { manager.run_attempt(attempt).await });
                    }
                }
            }
        }
    }

    /// Takes the next direct-first FIFO row when capacity is available.
    fn take_next(&self) -> Option<AdmittedAttempt> {
        let mut state = self.inner.state.lock();
        if state.active.len() >= state.max_concurrent {
            return None;
        }
        let id = take_queued_id(&mut state)?;
        let row = state.rows.get_mut(&id)?;
        row.view.status = DownloadStatus::Resolving;
        row.view.failure = None;
        let song = row.view.song.as_ref().clone();
        let quality = row.view.quality;
        let cancellation = CancellationToken::new();
        state.active.insert(id.clone(), cancellation.clone());
        mineral_log::info!(
            target: "download",
            download_id = %id,
            song_id = %song.id.qualified(),
            source = song.source().name(),
            "download attempt started"
        );
        Some(AdmittedAttempt {
            id,
            song,
            quality,
            cancellation,
        })
    }

    /// Executes one download, waits for its writer, then records the terminal result.
    async fn run_attempt(&self, attempt: AdmittedAttempt) {
        let manager = self.clone();
        let id = attempt.id.clone();
        let reporter = Arc::new(move |update| manager.report(&id, update));
        let outcome = match self.inner.runtime.music_dir.as_deref() {
            Some(music_dir) => {
                download_song(
                    &self.inner.runtime.playback,
                    &DownloadEnv {
                        music_dir,
                        hooks: &self.inner.runtime.hooks,
                    },
                    &attempt.song,
                    attempt.quality,
                    DownloadAttempt {
                        id: &attempt.id,
                        cancellation: &attempt.cancellation,
                    },
                    reporter,
                    self.inner.runtime.speed_tick,
                )
                .await
            }
            None => Err(color_eyre::eyre::eyre!(
                "download export directory is unavailable"
            )),
        };
        self.finish_attempt(&attempt, outcome);
    }

    /// Applies progress only while this download is active and has not accepted Stop.
    fn report(&self, id: &DownloadId, update: TransferUpdate) {
        let mut state = self.inner.state.lock();
        if !state.active.contains_key(id) {
            return;
        }
        let Some(row) = state.rows.get_mut(id) else {
            return;
        };
        if row.view.status == DownloadStatus::Stopping {
            return;
        }
        match update {
            TransferUpdate::Downloading {
                quality,
                bytes_done,
                bytes_total,
                speed_bps,
            } => {
                row.view.status = DownloadStatus::Downloading;
                row.view.quality = quality;
                row.view.bytes_done = bytes_done;
                row.view.bytes_total = bytes_total;
                row.view.speed_bps = speed_bps;
            }
            TransferUpdate::Finalizing => {
                row.view.status = DownloadStatus::Finalizing;
                row.view.speed_bps = 0;
            }
        }
    }

    /// Reconciles one completion and runs after-commit side effects outside the state lock.
    fn finish_attempt(
        &self,
        attempt: &AdmittedAttempt,
        outcome: color_eyre::Result<DownloadOutcome>,
    ) {
        let effect = match outcome {
            Ok(DownloadOutcome::Downloaded {
                path,
                quality,
                format,
                hooked,
            }) => CompletionEffect::Downloaded {
                path,
                quality,
                format,
                hooked,
            },
            Ok(DownloadOutcome::Skipped { cause, quality }) => {
                CompletionEffect::Skipped { cause, quality }
            }
            Err(_error) if attempt.cancellation.is_cancelled() => CompletionEffect::Stopped,
            Err(error) => CompletionEffect::Failed {
                failure: mineral_log::chain(&error),
            },
        };
        let status = effect.status();
        {
            let mut state = self.inner.state.lock();
            if state.active.remove(&attempt.id).is_none() {
                return;
            }
            state.mark_terminal(&attempt.id, status, effect.failure());
            state.finish_wave_if_idle();
        }
        self.run_completion_effect(attempt, effect);
        self.inner.wake.notify_one();
        self.inner.quiesced.notify_waiters();
    }

    /// Emits logs, events, tags, and stats implied by a committed terminal result.
    fn run_completion_effect(&self, attempt: &AdmittedAttempt, effect: CompletionEffect) {
        match effect {
            CompletionEffect::Downloaded {
                path,
                quality,
                format,
                hooked,
            } => {
                self.inner.runtime.notify.download_completed(
                    &attempt.song,
                    &path,
                    quality,
                    format.as_ref(),
                );
                self.inner
                    .runtime
                    .tagging
                    .enqueue(attempt.song.clone(), path.clone(), quality);
                record_download(
                    &self.inner.runtime.stats,
                    &attempt.song,
                    quality.as_str(),
                    format.as_ref().map(mineral_model::AudioFormat::as_str),
                    mineral_stats::DownloadOutcome::Downloaded,
                    hooked,
                    Some(path.display().to_string()),
                );
                mineral_log::info!(
                    target: "download",
                    download_id = %attempt.id,
                    song_id = %attempt.song.id.qualified(),
                    source = attempt.song.source().name(),
                    path = %path.display(),
                    "download export committed"
                );
            }
            CompletionEffect::Skipped { cause, quality } => {
                mineral_log::info!(
                    target: "download",
                    download_id = %attempt.id,
                    song_id = %attempt.song.id.qualified(),
                    source = attempt.song.source().name(),
                    reason = ?cause,
                    "download skipped"
                );
                let hooked = match cause {
                    SkipCause::AlreadyExists => mineral_stats::DownloadHook::None,
                    SkipCause::HookVeto => mineral_stats::DownloadHook::Skip,
                };
                record_download(
                    &self.inner.runtime.stats,
                    &attempt.song,
                    quality.as_str(),
                    None,
                    mineral_stats::DownloadOutcome::Skipped,
                    hooked,
                    None,
                );
            }
            CompletionEffect::Failed { failure } => {
                mineral_log::warn!(
                    target: "download",
                    download_id = %attempt.id,
                    song_id = %attempt.song.id.qualified(),
                    source = attempt.song.source().name(),
                    error = failure,
                    "download failed"
                );
                record_download(
                    &self.inner.runtime.stats,
                    &attempt.song,
                    attempt.quality.as_str(),
                    None,
                    mineral_stats::DownloadOutcome::Failed,
                    mineral_stats::DownloadHook::None,
                    None,
                );
            }
            CompletionEffect::Stopped => {
                mineral_log::info!(
                    target: "download",
                    download_id = %attempt.id,
                    song_id = %attempt.song.id.qualified(),
                    source = attempt.song.source().name(),
                    "download Stop quiesced"
                );
            }
        }
    }
}

/// Pops the next direct-first queued identity without aliasing state fields.
fn take_queued_id(state: &mut ManagerState) -> Option<DownloadId> {
    pop_queued(&mut state.direct_queue, &state.rows)
        .or_else(|| pop_queued(&mut state.playlist_queue, &state.rows))
}
