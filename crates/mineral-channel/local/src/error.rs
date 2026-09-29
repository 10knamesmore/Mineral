//! Preserves local file paths and causes across catalog and playback operations.

use std::path::PathBuf;

use mineral_model::SongId;

use crate::metadata::LyricsEncodingError;

/// Failures while scanning, projecting or reading the local library.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// A tilde root needs a home directory to expand.
    #[error("HOME unavailable for local root {}", path.display())]
    HomeUnavailable {
        /// Configured root before tilde expansion.
        path: PathBuf,
    },

    /// Library roots must be absolute after tilde expansion.
    #[error("local root must be absolute: {}", path.display())]
    RelativeRoot {
        /// Root after tilde expansion.
        path: PathBuf,
    },

    /// The configured root exists but is not a directory.
    #[error("local root is not a directory: {}", path.display())]
    RootNotDirectory {
        /// Canonical root that cannot be traversed as a directory.
        path: PathBuf,
    },

    /// A filesystem operation failed at the recorded path.
    #[error("{operation} {}", path.display())]
    File {
        /// Filesystem operation that failed.
        operation: &'static str,

        /// File or directory used by the operation.
        path: PathBuf,

        /// Original filesystem error.
        #[source]
        source: std::io::Error,
    },

    /// Directory traversal failed; WalkDir retains the affected path.
    #[error("scan local library")]
    Walk(#[from] walkdir::Error),

    /// An audio file could not be parsed for tags and media properties.
    #[error("probe local audio {}", path.display())]
    Probe {
        /// Audio file being inspected.
        path: PathBuf,

        /// Original tag or container parsing error.
        #[source]
        source: lofty::error::LoftyError,
    },

    /// MP4 probing or packet reading failed.
    #[error("{operation} {}", path.display())]
    Mp4 {
        /// Duration probing stage that failed.
        operation: &'static str,

        /// MP4 file being inspected.
        path: PathBuf,

        /// Original demuxer error.
        #[source]
        source: symphonia::core::errors::Error,
    },

    /// A packet's end timestamp exceeds the container's integer range.
    #[error("MP4 timestamp overflow in {}: {timestamp} + {duration}", path.display())]
    Mp4TimestampOverflow {
        /// MP4 file containing the packet.
        path: PathBuf,

        /// Packet start in track time-base ticks.
        timestamp: u64,

        /// Packet duration in the same ticks.
        duration: u64,
    },

    /// An untagged audio file has no usable filename stem.
    #[error("audio filename has no stem: {}", path.display())]
    MissingFilename {
        /// Audio path with no filename stem.
        path: PathBuf,
    },

    /// A discovered song cannot be assigned to a directory playlist.
    #[error("local song has no parent directory: {}", path.display())]
    MissingParent {
        /// Audio path with no parent directory.
        path: PathBuf,
    },

    /// A metadata value exceeds the common model's numeric range.
    #[error("{field} exceeds supported range in {}", path.display())]
    NumberOverflow {
        /// Metadata value being converted.
        field: &'static str,

        /// Audio file supplying the value.
        path: PathBuf,

        /// Original integer conversion error.
        #[source]
        source: std::num::TryFromIntError,
    },

    /// Sidecar text is invalid for its declared encoding.
    #[error("decode lyrics {}", path.display())]
    LyricsEncoding {
        /// Sidecar whose bytes could not be decoded.
        path: PathBuf,

        /// Invalid byte sequence or incomplete code unit.
        #[source]
        source: LyricsEncodingError,
    },

    /// The blocking scanner stopped before returning a catalog.
    #[error("local library scan worker failed")]
    ScanWorker(#[from] tokio::task::JoinError),

    /// The scanned metadata could not be stored.
    #[error("store local library metadata")]
    Storage(#[from] mineral_channel_core::store::StoreError),

    /// The requested identity is absent from this library's catalog.
    #[error("unknown local song {}", id.qualified())]
    SongNotFound {
        /// Playback identity absent from the current catalog.
        id: SongId,
    },
}

/// Local operations retain their domain error until crossing a trait boundary.
pub(crate) type Result<T> = std::result::Result<T, Error>;

impl From<Error> for mineral_channel_core::Error {
    fn from(error: Error) -> Self {
        match error {
            Error::SongNotFound { .. } => Self::NotFound,
            Error::Probe { .. }
            | Error::Mp4 { .. }
            | Error::Mp4TimestampOverflow { .. }
            | Error::MissingFilename { .. }
            | Error::MissingParent { .. }
            | Error::NumberOverflow { .. }
            | Error::LyricsEncoding { .. } => Self::Parse {
                source: Box::new(error),
            },
            Error::HomeUnavailable { .. }
            | Error::RelativeRoot { .. }
            | Error::RootNotDirectory { .. }
            | Error::File { .. }
            | Error::Walk(_)
            | Error::ScanWorker(_)
            | Error::Storage(_) => Self::Storage {
                source: Box::new(error),
            },
        }
    }
}

impl From<Error> for mineral_playback::Error {
    fn from(source: Error) -> Self {
        Self::Provider {
            source: Box::new(source),
        }
    }
}
