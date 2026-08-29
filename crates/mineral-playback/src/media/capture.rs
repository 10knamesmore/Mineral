//! Post-preparation media capture targets and completion receipts.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

/// Filesystem destination for one complete decoder-ready encoded media instance.
#[derive(Clone, Debug)]
pub struct CaptureTarget {
    /// Unique temporary path owned by this capture attempt.
    path: PathBuf,
}

impl CaptureTarget {
    /// Creates a capture target.
    ///
    /// # Params:
    ///   - `path`: Unique temporary path for the prepared media bytes.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Returns the temporary output path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// A complete capture that may be moved into the media cache.
pub struct CapturedMedia {
    /// Verified complete temporary file.
    path: PathBuf,

    /// Encoded byte length stored in the file.
    bytes: u64,
}

impl CapturedMedia {
    /// Creates a verified capture result.
    ///
    /// # Params:
    ///   - `path`: Complete temporary file.
    ///   - `bytes`: Encoded byte length stored in the file.
    #[must_use]
    pub fn new(path: PathBuf, bytes: u64) -> Self {
        Self { path, bytes }
    }

    /// Returns the encoded byte length.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Consumes the result and returns its temporary file path.
    #[must_use]
    pub fn into_path(self) -> PathBuf {
        self.path
    }
}

/// Single-use completion receipt for a producer-owned capture.
///
/// Providers create the receipt from their native completion signal after arranging for
/// post-preparation bytes to reach the requested [`CaptureTarget`]. Consumers await it before
/// moving the file into durable cache storage.
pub struct CaptureReceipt {
    /// Provider-owned completion and integrity verification.
    completion: Pin<Box<dyn Future<Output = color_eyre::Result<CapturedMedia>> + Send>>,
}

impl CaptureReceipt {
    /// Wraps a provider completion future.
    ///
    /// # Params:
    ///   - `completion`: Future that resolves only after the target file is complete and verified.
    #[must_use]
    pub fn new<F>(completion: F) -> Self
    where
        F: Future<Output = color_eyre::Result<CapturedMedia>> + Send + 'static,
    {
        Self {
            completion: Box::pin(completion),
        }
    }

    /// Waits for the producer-owned capture to finish.
    ///
    /// # Return:
    ///   A verified complete media file.
    ///
    /// # Error:
    ///   Returns the producer, cancellation, filesystem, or integrity failure.
    pub async fn wait(self) -> color_eyre::Result<CapturedMedia> {
        self.completion.await
    }
}
