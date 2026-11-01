//! The download task owns admission, progress, completion, and graceful writer shutdown.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use mineral_model::{BitRate, Song};
use mineral_protocol::{DownloadId, DownloadOrigin, DownloadStatus, DownloadTarget};
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, sleep};
use tokio_util::sync::CancellationToken;

use super::admission::{Lane, admit_song, apply_playlist, expand_playlist};
use super::completion::finish_attempt;
use super::lifecycle::{Command, DownloadRuntime, stop_all, stop_download};
use super::state::{DownloadSnapshot, ManagerState, pop_queued};
use crate::download::{
    DownloadAttempt, DownloadEnv, DownloadOutcome, TransferError, TransferUpdate, download_song,
};

/// Coalesces progress before cloning and sorting the client-facing rows.
const SNAPSHOT_DELAY: Duration = Duration::from_millis(100);

/// One admitted execution; its result is consumed once by the manager task.
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

/// One FIFO carries progress and completion so terminal rows retain the final byte counts.
enum AttemptEvent {
    Progress {
        id: DownloadId,

        update: TransferUpdate,
    },

    Finished {
        attempt: Box<AdmittedAttempt>,

        outcome: Result<DownloadOutcome, TransferError>,
    },
}

/// Owns all state transitions; shutdown drops playlist queries but drains download writers.
pub(super) async fn run_downloads(
    runtime: DownloadRuntime,
    mut state: ManagerState,
    mut commands: mpsc::UnboundedReceiver<Command>,
    snapshots: watch::Sender<DownloadSnapshot>,
) {
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let mut attempts = FuturesUnordered::new();
    let mut playlists = FuturesUnordered::new();
    let publish = sleep(SNAPSHOT_DELAY);
    tokio::pin!(publish);
    let mut changed = false;

    let shutdown_reply = loop {
        while let Some(attempt) = take_next(&mut state) {
            attempts.push(run_attempt(&runtime, attempt, events_tx.clone()));
        }
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Submit(DownloadTarget::Song(song))) => {
                    admit_song(&mut state, &runtime, &song, DownloadOrigin::Direct, Lane::Direct);
                }
                Some(Command::Submit(DownloadTarget::Playlist(id))) => {
                    state.preparing_playlists += 1;
                    playlists.push(expand_playlist(&runtime, id));
                }
                Some(Command::Configure { quality, max_concurrent }) => {
                    state.quality = quality;
                    state.max_concurrent = max_concurrent;
                }
                Some(Command::Stop { id, reply }) => {
                    let _ = reply.send(stop_download(&mut state, &id));
                }
                Some(Command::Shutdown(reply)) => break Some(reply),
                None => break None,
            },
            Some(event) = events_rx.recv() => {
                apply_attempt_event(&mut state, &runtime, event);
            }
            Some(()) = attempts.next(), if !attempts.is_empty() => {
                continue;
            }
            Some((id, result)) = playlists.next(), if !playlists.is_empty() => {
                apply_playlist(&mut state, &runtime, id, result);
            }
            () = &mut publish, if changed => {
                snapshots.send_replace(state.snapshot());
                changed = false;
                continue;
            }
        }
        state.finish_wave_if_idle();
        if !changed {
            publish.as_mut().reset(Instant::now() + SNAPSHOT_DELAY);
            changed = true;
        }
    };

    // Reject further commands, cancel catalog queries, and keep polling downloads until writers exit.
    drop(commands);
    drop(playlists);
    state.preparing_playlists = 0;
    stop_all(&mut state);
    state.finish_wave_if_idle();
    snapshots.send_replace(state.snapshot());
    while !state.active.is_empty() {
        tokio::select! {
            Some(event) = events_rx.recv() => apply_attempt_event(&mut state, &runtime, event),
            Some(()) = attempts.next(), if !attempts.is_empty() => continue,
        }
        state.finish_wave_if_idle();
        snapshots.send_replace(state.snapshot());
    }
    if let Some(reply) = shutdown_reply {
        let _ = reply.send(());
    }
}

/// Takes the next direct-first FIFO row when capacity is available.
fn take_next(state: &mut ManagerState) -> Option<AdmittedAttempt> {
    if state.active.len() >= state.max_concurrent {
        return None;
    }
    let id = pop_queued(&mut state.direct_queue, &state.rows)
        .or_else(|| pop_queued(&mut state.playlist_queue, &state.rows))?;
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

/// Waits for the transfer and its writer; the reporter only sends progress messages.
async fn run_attempt(
    runtime: &DownloadRuntime,
    attempt: AdmittedAttempt,
    events: mpsc::UnboundedSender<AttemptEvent>,
) {
    let id = attempt.id.clone();
    let progress = events.clone();
    let reporter = Arc::new(move |update| {
        let _ = progress.send(AttemptEvent::Progress {
            id: id.clone(),
            update,
        });
    });
    let outcome = match runtime.music_dir.as_deref() {
        Some(music_dir) => {
            download_song(
                &runtime.playback,
                &DownloadEnv {
                    music_dir,
                    hooks: &runtime.hooks,
                },
                &attempt.song,
                attempt.quality,
                DownloadAttempt {
                    id: &attempt.id,
                    cancellation: &attempt.cancellation,
                },
                reporter,
                runtime.speed_tick,
            )
            .await
        }
        None => Err(TransferError::Unavailable),
    };
    let _ = events.send(AttemptEvent::Finished {
        attempt: Box::new(attempt),
        outcome,
    });
}

fn apply_attempt_event(state: &mut ManagerState, runtime: &DownloadRuntime, event: AttemptEvent) {
    match event {
        AttemptEvent::Progress { id, update } => report(state, &id, update),
        AttemptEvent::Finished { attempt, outcome } => {
            finish_attempt(state, runtime, &attempt, outcome);
        }
    }
}

/// Keeps transfer facts current while stopping, without restoring active status or speed.
fn report(state: &mut ManagerState, id: &DownloadId, update: TransferUpdate) {
    if !state.active.contains_key(id) {
        return;
    }
    let Some(row) = state.rows.get_mut(id) else {
        return;
    };
    match update {
        TransferUpdate::Downloading {
            quality,
            bytes_done,
            bytes_total,
            speed_bps,
        } => {
            row.view.quality = quality;
            row.view.bytes_done = bytes_done;
            row.view.bytes_total = bytes_total;
            if row.view.status != DownloadStatus::Stopping {
                row.view.status = DownloadStatus::Downloading;
                row.view.speed_bps = speed_bps;
            }
        }
        TransferUpdate::Finalizing => {
            if row.view.status != DownloadStatus::Stopping {
                row.view.status = DownloadStatus::Finalizing;
            }
            row.view.speed_bps = 0;
        }
    }
}
