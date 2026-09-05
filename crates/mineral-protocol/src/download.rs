//! Session-only manual download protocol types.

use mineral_model::{BitRate, PlaylistId, Song};
use serde::{Deserialize, Serialize};

use crate::PlaylistRef;

/// Stable identity of one Song download during the current daemon session.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DownloadId(String);

impl DownloadId {
    /// Creates an identity from its process-generated value.
    ///
    /// # Params:
    ///   - `value`: Unique opaque value.
    #[must_use]
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Returns the opaque identity value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DownloadId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Download target submitted by a client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DownloadTarget {
    /// One Song. `Box` keeps the enclosing request enum compact.
    Song(Box<Song>),

    /// Every entry in the canonical playlist snapshot.
    Playlist(PlaylistId),
}

/// Read-only provenance of one Song download.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DownloadOrigin {
    /// Submitted directly from a Song selection.
    Direct,

    /// Expanded from a playlist snapshot.
    Playlist(PlaylistRef),
}

/// Lifecycle state of one Song download.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DownloadStatus {
    /// Waiting for an execution slot.
    Queued,

    /// Resolving and opening provider media.
    Resolving,

    /// Draining encoded media to a unique partial file.
    Downloading,

    /// Installing the completed partial as the final export.
    Finalizing,

    /// Cancellation was accepted and the active writer is quiescing.
    Stopping,

    /// Stopped by the user before export commit.
    Stopped,

    /// Permanently exported in this attempt.
    Downloaded,

    /// A matching permanent export already existed.
    AlreadyPresent,

    /// Rejected by the `before_download` hook.
    SkippedByHook,

    /// Provider, reader, or filesystem work failed.
    Failed,
}

impl DownloadStatus {
    /// Whether this state can accept Stop.
    #[must_use]
    pub fn stoppable(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Resolving | Self::Downloading | Self::Finalizing | Self::Stopping
        )
    }

    /// Whether this state has no further lifecycle transition.
    #[must_use]
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Stopped
                | Self::Downloaded
                | Self::AlreadyPresent
                | Self::SkippedByHook
                | Self::Failed
        )
    }
}

/// Flat client snapshot of one Song download.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SongDownloadView {
    /// Session-local download identity.
    pub id: DownloadId,

    /// Song metadata captured at admission.
    pub song: Box<Song>,

    /// Direct or playlist expansion provenance.
    pub origin: DownloadOrigin,

    /// Current lifecycle state.
    pub status: DownloadStatus,

    /// Requested quality, updated to the effective quality after media open.
    pub quality: BitRate,

    /// Bytes written to the owned partial file.
    pub bytes_done: u64,

    /// Provider-declared total bytes, or `None` when unavailable.
    pub bytes_total: Option<u64>,

    /// Smoothed current transfer rate in bytes per second.
    pub speed_bps: u64,

    /// Full failure chain for a failed row.
    pub failure: Option<String>,
}

/// Result counts for the latest settled wave of Song downloads.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadWave {
    /// Monotonic process-local sequence used by clients to show one flash per wave.
    pub sequence: u64,

    /// Newly exported songs.
    pub downloaded: usize,

    /// Songs skipped because a matching export already existed.
    pub already_present: usize,

    /// Songs rejected by the download hook.
    pub skipped_by_hook: usize,

    /// Failed songs.
    pub failed: usize,

    /// User-stopped songs.
    pub stopped: usize,
}

/// Small download snapshot polled by every client tick.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadSummary {
    /// Active Song attempts.
    pub active: usize,

    /// Songs waiting in either admission lane.
    pub queued: usize,

    /// Playlist snapshots currently being expanded.
    pub preparing_playlists: usize,

    /// Aggregate transfer rate of active songs in bytes per second.
    pub speed_bps: u64,

    /// Latest settled wave, retained until superseded.
    pub latest_wave: Option<DownloadWave>,
}
