//! Terminal download effects outside the lifecycle state lock.

use std::path::PathBuf;

use mineral_model::{BitRate, Song};
use mineral_protocol::DownloadStatus;

use super::super::{DownloadOutcome, SkipCause};
use super::DownloadManager;
use super::scheduler::AdmittedAttempt;

/// Completion side effects detached from state mutation.
pub(super) enum CompletionEffect {
    /// Final export committed.
    Downloaded {
        /// Final path.
        path: PathBuf,

        /// Effective quality.
        quality: BitRate,

        /// Effective container format.
        format: Option<mineral_model::AudioFormat>,

        /// Hook decision recorded in stats.
        hooked: mineral_stats::DownloadHook,
    },

    /// Existing export or hook veto.
    Skipped {
        /// Why no export was committed.
        cause: SkipCause,

        /// Quality identity used by the skip decision.
        quality: BitRate,
    },

    /// Transfer failure.
    Failed {
        /// Human-readable full error chain.
        failure: String,
    },

    /// Cooperative Stop before export commit.
    Stopped,
}

impl CompletionEffect {
    /// Terminal status represented by this effect.
    pub(super) fn status(&self) -> DownloadStatus {
        match self {
            Self::Downloaded { .. } => DownloadStatus::Downloaded,
            Self::Skipped {
                cause: SkipCause::AlreadyExists,
                ..
            } => DownloadStatus::AlreadyPresent,
            Self::Skipped {
                cause: SkipCause::HookVeto,
                ..
            } => DownloadStatus::SkippedByHook,
            Self::Failed { .. } => DownloadStatus::Failed,
            Self::Stopped => DownloadStatus::Stopped,
        }
    }

    /// Failure text for the client row.
    pub(super) fn failure(&self) -> Option<String> {
        match self {
            Self::Failed { failure } => Some(failure.clone()),
            Self::Downloaded { .. } | Self::Skipped { .. } | Self::Stopped => None,
        }
    }
}

impl DownloadManager {
    /// Reconciles one completion and runs after-commit side effects outside the state lock.
    pub(super) fn finish_attempt(
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
        self.bump();
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

/// Records one terminal download result.
#[allow(clippy::too_many_arguments)] // The stats event has these independent columns.
pub(super) fn record_download(
    stats: &crate::StatsRecorder,
    song: &Song,
    quality: &str,
    format: Option<&str>,
    outcome: mineral_stats::DownloadOutcome,
    hooked: mineral_stats::DownloadHook,
    path: Option<String>,
) {
    stats.event(mineral_stats::StatsEvent::Behavior {
        actor: mineral_stats::Actor::System,
        event: mineral_stats::BehaviorEvent::Download {
            song: song.id.clone(),
            quality: quality.to_owned(),
            format: format.map(str::to_owned),
            outcome,
            hooked,
            path,
        },
    });
}
