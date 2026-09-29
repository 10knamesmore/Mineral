//! Playback resolution, media opening, and capture failures.

use std::path::PathBuf;

use mineral_model::SourceKind;
use thiserror::Error;
use url::Url;

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

    /// The playback instance was cancelled before opening completed.
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

    /// An HTTP media request failed before receiving a response.
    #[error("request media {url}")]
    HttpRequest {
        /// Requested media URL.
        url: Url,

        /// HTTP failure.
        #[source]
        source: reqwest::Error,
    },

    /// The HTTP server returned a non-success status.
    #[error("media response {url} returned {status}")]
    HttpStatus {
        /// Requested media URL.
        url: Url,

        /// HTTP status code.
        status: reqwest::StatusCode,

        /// HTTP response failure.
        #[source]
        source: reqwest::Error,
    },

    /// Reading the HTTP response body failed.
    #[error("read media response body {url}")]
    HttpBody {
        /// Requested media URL.
        url: Url,

        /// HTTP body stream failure.
        #[source]
        source: reqwest::Error,
    },

    /// Building the reusable media client failed.
    #[error("build media client")]
    HttpClient {
        /// HTTP client construction failure.
        #[source]
        source: reqwest::Error,
    },

    /// Creating stream-download storage failed.
    #[error("create media buffer")]
    BufferStorage {
        /// Storage creation failure.
        #[source]
        source: std::io::Error,
    },

    /// The capture producer stopped before reaching completion.
    #[error("capture producer ended before download completed")]
    CaptureIncomplete,

    /// Capture file metadata could not be read.
    #[error("read capture metadata {}", path.display())]
    CaptureMetadata {
        /// Capture file path.
        path: PathBuf,

        /// Filesystem failure.
        #[source]
        source: std::io::Error,
    },

    /// The capture file has fewer bytes than the declared media length.
    #[error("capture truncated at {}: {bytes} / {expected} bytes", path.display())]
    CaptureTruncated {
        /// Capture file path.
        path: PathBuf,

        /// Available encoded bytes.
        bytes: u64,

        /// Expected encoded bytes.
        expected: u64,
    },
}
