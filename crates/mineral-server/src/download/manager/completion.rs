//! Terminal download effects outside the lifecycle state lock.

use std::path::PathBuf;

use mineral_model::{BitRate, Song};
use mineral_protocol::DownloadStatus;

use super::super::SkipCause;

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
