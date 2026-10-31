//! Writes completed in-memory downloads without making playback depend on file I/O.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use tempfile::NamedTempFile;
use thiserror::Error;
use tokio::task::{JoinError, spawn_blocking};

use super::buffer::DownloadBuffer;
use crate::{CaptureReceipt, CaptureTarget, CapturedMedia};

/// Failures while saving a completed download as a capture file.
#[derive(Debug, Error)]
pub enum CaptureError {
    /// File creation, writing, or publication failed.
    #[error("write capture {}", path.display())]
    Write {
        /// Capture destination.
        path: PathBuf,

        /// Filesystem failure.
        #[source]
        source: io::Error,
    },

    /// The blocking file writer did not finish normally.
    #[error("capture writer task failed")]
    Task(#[from] JoinError),
}

/// Capture shares immutable chunks but not the reader's lifetime guard.
pub(super) fn capture(buffer: Arc<DownloadBuffer>, target: CaptureTarget) -> CaptureReceipt {
    CaptureReceipt::new(async move {
        let chunks = buffer.completed_chunks().await?;
        // A completed download can be cached even if its playback reader is now gone.
        drop(buffer);
        let captured = spawn_blocking(move || write_capture(&chunks, target.path().to_path_buf()))
            .await
            .map_err(CaptureError::from)??;
        Ok(captured)
    })
}

fn write_capture(chunks: &[Bytes], path: PathBuf) -> Result<CapturedMedia, CaptureError> {
    let write_error = |source| CaptureError::Write {
        path: path.clone(),
        source,
    };
    let parent = path.parent().ok_or_else(|| {
        write_error(io::Error::new(
            io::ErrorKind::InvalidInput,
            "capture path has no parent directory",
        ))
    })?;
    fs::create_dir_all(parent).map_err(write_error)?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(write_error)?;
    for chunk in chunks {
        temporary.write_all(chunk).map_err(write_error)?;
    }
    temporary.as_file().sync_all().map_err(write_error)?;
    let bytes = temporary.as_file().metadata().map_err(write_error)?.len();
    temporary
        .persist(&path)
        .map_err(|error| write_error(error.error))?;
    Ok(CapturedMedia::new(path, bytes))
}
