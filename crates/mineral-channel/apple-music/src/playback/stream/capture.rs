//! Assembles ready memory segments into a temporary file, then atomically publishes the capture.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use mineral_playback::{CaptureReceipt, CaptureTarget, CapturedMedia};
use tempfile::NamedTempFile;

use super::export::write_mp4;
use super::segments::{PreparedSegment, SegmentStore};
use crate::playback::error::{Error, Result};

/// Capture observes the reader-owned cancellation root without retaining its DropGuard.
/// An unfinished track is discarded when playback stops rather than downloaded for capture.
pub(in crate::playback) fn capture(
    store: Arc<SegmentStore>,
    target: &CaptureTarget,
) -> CaptureReceipt {
    let destination = target.path().to_path_buf();
    let worker = tokio::spawn(async move {
        let prepared = store.complete_segments(store.cancellation.clone()).await?;
        tokio::task::spawn_blocking(move || capture_file(&store, &prepared, destination)).await?
    });
    CaptureReceipt::new(async move { worker.await.map_err(Error::from)?.map_err(Into::into) })
}

fn capture_file(
    store: &SegmentStore,
    prepared: &[Arc<PreparedSegment>],
    destination: PathBuf,
) -> Result<CapturedMedia> {
    if store.cancellation.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let parent = destination.parent().ok_or(Error::InvalidMedia {
        field: "capture directory",
    })?;
    fs::create_dir_all(parent)?;
    let mut temporary = NamedTempFile::new_in(parent)?;
    let bytes = write_mp4(store, prepared, temporary.as_file_mut())?;
    temporary.as_file().sync_all()?;
    if store.cancellation.is_cancelled() {
        return Err(Error::Cancelled);
    }
    temporary
        .persist(&destination)
        .map_err(|error| Error::Io(error.error))?;
    Ok(CapturedMedia::new(destination, bytes))
}
