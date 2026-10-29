//! Copies the complete encoded file to a capture destination and atomically publishes it.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use mineral_playback::{CaptureReceipt, CaptureTarget, CapturedMedia};
use tempfile::NamedTempFile;
use tokio_util::sync::DropGuard;

use super::reader::CompleteFileReader;
use super::segments::SegmentStore;
use crate::playback::error::{Error, Result};

/// Capture is a complete-file consumer and keeps the playback root alive until its copy ends.
pub(in crate::playback) fn capture(
    store: Arc<SegmentStore>,
    target: &CaptureTarget,
    lifetime: Arc<DropGuard>,
) -> CaptureReceipt {
    let destination = target.path().to_path_buf();
    let worker = tokio::task::spawn_blocking(move || capture_file(store, destination, lifetime));
    CaptureReceipt::new(async move { worker.await.map_err(Error::from)?.map_err(Into::into) })
}

fn capture_file(
    store: Arc<SegmentStore>,
    destination: PathBuf,
    lifetime: Arc<DropGuard>,
) -> Result<CapturedMedia> {
    let parent = destination.parent().ok_or(Error::InvalidMedia {
        field: "capture directory",
    })?;
    fs::create_dir_all(parent)?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    let mut reader = CompleteFileReader::new(store, None, lifetime);
    let bytes = io::copy(&mut reader, temporary.as_file_mut())?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(&destination)
        .map_err(|error| Error::Io(error.error))?;
    Ok(CapturedMedia::new(destination, bytes))
}
