//! Assembles complete MP4 output from shared memory segments.
//! Capture assembles from existing segment buffers; encoded-file readers materialize bytes on demand.

use std::io::{Cursor, Seek, Write};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::super::error::{Error, Result};
use super::super::mp4::Mp4Writer;
use super::segments::{PreparedSegment, SegmentStore};

/// The cell belongs to the store, so every complete-file consumer shares its result.
/// Initialization runs in the waiting future: cancelling it before completion leaves
/// the cell uninitialized for the next consumer. Root cancellation also stops segment workers.
pub(super) async fn complete_bytes(
    store: &Arc<SegmentStore>,
    request: CancellationToken,
) -> Result<Arc<[u8]>> {
    tokio::select! {
        biased;
        () = request.cancelled() => Err(Error::Cancelled),
        () = store.cancellation.cancelled() => Err(Error::Cancelled),
        result = store.encoded_file.get_or_init(|| async {
            build_bytes(store).await.map_err(Arc::new)
        }) => result.clone().map_err(Error::Preparation),
    }
}

async fn build_bytes(store: &Arc<SegmentStore>) -> Result<Arc<[u8]>> {
    let prepared = store.complete_segments(store.cancellation.clone()).await?;
    let store = store.clone();
    tokio::task::spawn_blocking(move || {
        let mut output = Cursor::new(Vec::new());
        write_mp4(&store, &prepared, &mut output)?;
        Ok(Arc::from(output.into_inner()))
    })
    .await?
}

/// Writes already prepared segments without building a second whole-track audio buffer.
/// Call on a blocking thread; cancellation is checked between segment writes.
pub(super) fn write_mp4(
    store: &SegmentStore,
    prepared: &[Arc<PreparedSegment>],
    output: &mut (impl Write + Seek),
) -> Result<u64> {
    if store.cancellation.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let mut muxer = Mp4Writer::new(output, &store.initialization)?;
    for segment in prepared {
        if store.cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let mut input = Cursor::new(&*segment.bytes);
        input.set_position(segment.payload_offset);
        muxer.append(output, &segment.fragment, &mut input)?;
    }
    let byte_len = muxer.finish(output)?;
    if store.cancellation.is_cancelled() {
        return Err(Error::Cancelled);
    }
    store.transfer.set_downloaded(byte_len);
    Ok(byte_len)
}
