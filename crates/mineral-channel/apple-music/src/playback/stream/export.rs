//! Assembles one complete regular MP4 from the same published segments used by playback.
//! Complete-file readers share the export result held by the segment store.

use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;

use tempfile::NamedTempFile;
use tokio_util::sync::CancellationToken;

use super::super::error::{Error, Result};
use super::super::mp4::Mp4Writer;
use super::segments::SegmentStore;

/// The cell belongs to the store, so every complete-file consumer shares its result.
/// Initialization runs in the waiting future: cancelling it before completion leaves
/// the cell uninitialized for the next consumer. Root cancellation also stops segment workers.
pub(super) async fn complete_file(
    store: &Arc<SegmentStore>,
    request: CancellationToken,
) -> Result<PathBuf> {
    tokio::select! {
        biased;
        () = request.cancelled() => Err(Error::Cancelled),
        () = store.cancellation.cancelled() => Err(Error::Cancelled),
        result = store.export.get_or_init(|| async {
            build_export(store).await.map_err(Arc::new)
        }) => result.clone().map_err(Error::Preparation),
    }
}

async fn build_export(store: &Arc<SegmentStore>) -> Result<PathBuf> {
    let mut temporary = NamedTempFile::new_in(store.directory.path())?;
    let mut muxer = Mp4Writer::new(temporary.as_file_mut(), &store.initialization)?;
    for index in 0..store.segment_count() {
        let prepared = store.segment(index, store.cancellation.clone()).await?;
        let mut input = File::open(&prepared.path)?;
        input.seek(SeekFrom::Start(prepared.payload_offset))?;
        muxer.append(temporary.as_file_mut(), &prepared.fragment, &mut input)?;
    }
    let byte_len = muxer.finish(temporary.as_file_mut())?;
    let path = store.directory.path().join("complete.mp4");
    temporary
        .persist(&path)
        .map_err(|error| Error::Io(error.error))?;
    store.transfer.set_downloaded(byte_len);
    Ok(path)
}
