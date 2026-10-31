//! Playback resolution, media opening, and capture failures.

use std::path::PathBuf;
use std::sync::Arc;

use mineral_model::SourceKind;
use thiserror::Error;

use crate::direct::{CaptureError, DownloadError};

/// Failures at the playback provider and decoder-ready media boundary.
#[derive(Debug, Error)]
pub enum Error {
    /// A source already has a registered playback provider.
    #[error("duplicate playback provider for {source_kind:?}")]
    DuplicateProvider {
        /// Source identity served by both providers.
        source_kind: SourceKind,
    },

    /// A source-specific provider failed while resolving or opening media.
    #[error("playback provider failed")]
    Provider {
        /// Original source-specific error, retained without depending on the provider crate.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The playback or download instance was cancelled.
    #[error("playback instance cancelled")]
    Cancelled,

    /// A direct media plan was consumed with the wrong locator kind.
    #[error("direct media locator is not {expected}")]
    InvalidLocator {
        /// Locator kind required by the opening operation.
        expected: &'static str,
    },

    /// A local media file could not be opened.
    #[error("open local media {}", path.display())]
    OpenLocal {
        /// Path to the local media.
        path: PathBuf,

        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },

    /// Acquisition failed; readers and capture retain the same original cause.
    #[error("download media")]
    Download {
        /// Acquisition failure.
        #[from]
        source: Arc<DownloadError>,
    },

    /// Writing or publishing the completed in-memory capture failed.
    #[error("capture media")]
    Capture {
        /// Capture output failure.
        #[from]
        source: CaptureError,
    },
}

impl From<DownloadError> for Error {
    fn from(source: DownloadError) -> Self {
        Self::Download {
            source: Arc::new(source),
        }
    }
}
