//! Bounded direct-first scheduling and progress for active download attempts.

use std::sync::Arc;

use mineral_model::{BitRate, Song};
use mineral_protocol::{DownloadId, DownloadStatus};
use tokio_util::sync::CancellationToken;

use super::DownloadManager;
use super::state::{ManagerState, pop_queued};
use crate::download::{DownloadAttempt, DownloadEnv, TransferUpdate, download_song};

/// One execution detached from the state lock; each DownloadId executes at most once.
pub(super) struct AdmittedAttempt {
    /// Download identity.
    pub(super) id: DownloadId,

    /// Song snapshot.
    pub(super) song: Song,

    /// Requested quality.
    pub(super) quality: BitRate,

    /// Cooperative cancellation handle.
    pub(super) cancellation: CancellationToken,
}

impl DownloadManager {
    /// Runs the bounded admission loop until graceful shutdown.
    pub(super) async fn run_scheduler(&self) {
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
    pub(super) fn take_next(&self) -> Option<AdmittedAttempt> {
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
    pub(super) fn report(&self, id: &DownloadId, update: TransferUpdate) {
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
        drop(state);
        self.bump();
    }
}

/// Pops the next direct-first queued identity without aliasing state fields.
fn take_queued_id(state: &mut ManagerState) -> Option<DownloadId> {
    pop_queued(&mut state.direct_queue, &state.rows)
        .or_else(|| pop_queued(&mut state.playlist_queue, &state.rows))
}
