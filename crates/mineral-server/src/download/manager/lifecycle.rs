//! Download commands and read-only snapshots; only the manager task changes lifecycle state.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mineral_channel_core::MusicChannel;
use mineral_model::BitRate;
use mineral_playback::PlaybackRegistry;
use mineral_protocol::{DownloadId, DownloadStatus, DownloadTarget};
use tokio::sync::{mpsc, oneshot, watch};

use super::scheduler::run_downloads;
use super::state::{DownloadSnapshot, ManagerState};

/// Failure to accept a Stop request.
#[derive(Debug, thiserror::Error)]
pub(crate) enum StopError {
    /// The requested identity is not retained by the manager.
    #[error("download identity not found")]
    NotFound,

    /// The manager has shut down before accepting the request.
    #[error("download manager unavailable")]
    Unavailable(#[from] oneshot::error::RecvError),
}

/// Immutable services used by download attempts and playlist expansion.
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

/// Commands processed in channel order by the download manager task.
pub(super) enum Command {
    Submit(DownloadTarget),

    Configure {
        quality: BitRate,

        max_concurrent: usize,
    },

    /// Acknowledges the state transition and cancellation request, not writer exit.
    Stop {
        id: DownloadId,

        reply: oneshot::Sender<Result<(), StopError>>,
    },

    /// Acknowledges only after all active attempts, including their writers, have exited.
    Shutdown(oneshot::Sender<()>),
}

/// Cloneable command sender and latest coalesced download snapshot.
#[derive(Clone)]
pub(crate) struct DownloadManager {
    commands: mpsc::UnboundedSender<Command>,

    snapshots: watch::Receiver<DownloadSnapshot>,
}

impl DownloadManager {
    /// Starts the lifecycle owner; config loading validates a positive concurrency limit.
    pub(crate) fn spawn(runtime: DownloadRuntime, quality: BitRate, max_concurrent: usize) -> Self {
        if let Some(root) = runtime.music_dir.as_deref() {
            crate::download::cleanup_orphan_partials(root);
        }
        let state = ManagerState::new(quality, max_concurrent);
        let (snapshots_tx, snapshots) = watch::channel(state.snapshot());
        let (commands, commands_rx) = mpsc::unbounded_channel();
        tokio::spawn(run_downloads(runtime, state, commands_rx, snapshots_tx));
        Self {
            commands,
            snapshots,
        }
    }

    /// Queues a Song submission or playlist expansion without waiting for admission.
    pub(crate) fn submit(&self, target: DownloadTarget) {
        let _ = self.commands.send(Command::Submit(target));
    }

    /// Changes future admissions; active attempts keep their quality and are not cancelled.
    pub(crate) fn set_config(&self, quality: BitRate, max_concurrent: usize) {
        let _ = self.commands.send(Command::Configure {
            quality,
            max_concurrent,
        });
    }

    /// Observes coalesced state without accessing the manager's mutable rows.
    pub(crate) fn snapshots(&self) -> watch::Receiver<DownloadSnapshot> {
        self.snapshots.clone()
    }

    /// Waits until Stop is accepted; running attempts may still be stopping on return.
    pub(crate) async fn stop(&self, id: &DownloadId) -> Result<(), StopError> {
        let (reply, accepted) = oneshot::channel();
        let _ = self.commands.send(Command::Stop {
            id: id.clone(),
            reply,
        });
        accepted.await?
    }

    /// Cancels work and waits for the manager to finish every active writer.
    pub(crate) async fn shutdown(&self) -> Result<(), oneshot::error::RecvError> {
        let (reply, finished) = oneshot::channel();
        let _ = self.commands.send(Command::Shutdown(reply));
        finished.await
    }
}

/// Stops queued work immediately or asks an active attempt to stop; terminal rows are unchanged.
pub(super) fn stop_download(state: &mut ManagerState, id: &DownloadId) -> Result<(), StopError> {
    let row = state.rows.get_mut(id).ok_or(StopError::NotFound)?;
    let song_id = row.view.song.id.clone();
    let queued = row.view.status == DownloadStatus::Queued;
    match row.view.status {
        DownloadStatus::Queued => state.mark_terminal(id, DownloadStatus::Stopped, None),
        DownloadStatus::Resolving | DownloadStatus::Downloading | DownloadStatus::Finalizing => {
            row.view.status = DownloadStatus::Stopping;
            row.view.speed_bps = 0;
            if let Some(token) = state.active.get(id) {
                token.cancel();
            }
        }
        DownloadStatus::Stopping
        | DownloadStatus::Stopped
        | DownloadStatus::Downloaded
        | DownloadStatus::AlreadyPresent
        | DownloadStatus::SkippedByHook
        | DownloadStatus::Failed => {}
    }
    mineral_log::info!(
        target: "download",
        download_id = %id,
        song_id = %song_id.qualified(),
        source = song_id.namespace().name(),
        queued,
        "download Stop accepted"
    );
    Ok(())
}

/// Stops admission before the manager drains its running futures.
pub(super) fn stop_all(state: &mut ManagerState) {
    let queued = state
        .rows
        .iter()
        .filter(|(_, row)| row.view.status == DownloadStatus::Queued)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for id in queued {
        state.mark_terminal(&id, DownloadStatus::Stopped, None);
    }
    for (id, token) in &state.active {
        if let Some(row) = state.rows.get_mut(id) {
            row.view.status = DownloadStatus::Stopping;
            row.view.speed_bps = 0;
        }
        token.cancel();
    }
}
