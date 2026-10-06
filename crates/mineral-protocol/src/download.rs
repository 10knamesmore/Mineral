//! Session-only manual download protocol types.

use mineral_model::{BitRate, PlaylistId, Song};
use serde::{Deserialize, Serialize};

/// Download provenance: playlist identity and its display name at admission.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlaylistRef {
    /// Playlist identity.
    pub id: PlaylistId,

    /// Display name.
    pub name: String,
}

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

/// 下载失败的可传输类别；原始原因留在 daemon 日志，客户端据此生成提示。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum DownloadFailure {
    /// 播放资源提供者或导出目录不可用。
    #[error("download resource unavailable")]
    Unavailable,

    /// 播放资源解析或准备失败。
    #[error("download media preparation failed")]
    Preparation,

    /// 已打开的媒体无法继续读取。
    #[error("download media read failed")]
    Read,

    /// 导出目录或文件写入失败。
    #[error("download storage failed")]
    Storage,

    /// 媒体字节数不合法，或内容短于声明长度。
    #[error("download media is incomplete or invalid")]
    InvalidMedia,

    /// 下载脚本改写没有提供可播放的媒体。
    #[error("download rewrite is invalid")]
    InvalidRewrite,

    /// 后台任务异常或内部计数无法表示。
    #[error("download worker failed")]
    Internal,
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

    /// 失败类别；非失败状态没有值，诊断细节保留在 daemon 日志。
    pub failure: Option<DownloadFailure>,
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
