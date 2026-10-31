//! Shared encoded bytes and interruptible reads for one remote track.

use std::collections::BTreeMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::Range;
use std::sync::Arc;

use bytes::Bytes;
use parking_lot::Mutex;
use rangemap::RangeSet;
use thiserror::Error;
use tokio::runtime::Handle;
use tokio::sync::Notify;
use tokio::{pin, select};
use tokio_util::sync::{CancellationToken, DropGuard};

use super::DownloadError;
use crate::{
    BufferedPrefix, Error as PlaybackError, ReadControl, Result as PlaybackResult, TransferState,
};

/// New decoder work superseded this read; this is not an acquisition failure.
#[derive(Debug, Error)]
#[error("media read superseded by a newer decoder operation")]
struct ReadInterrupted;

struct DownloadState {
    chunks: BTreeMap<u64, Bytes>,

    available: RangeSet<u64>,

    length: Option<u64>,

    requested: Option<Range<u64>>,

    /// HTTP completion result; cancellation is carried by the playback lifetime token.
    completion: Option<Result<(), Arc<DownloadError>>>,
}

/// One producer writes immutable chunks; playback and capture share their allocations.
pub(super) struct DownloadBuffer {
    state: Mutex<DownloadState>,

    changed: Notify,

    pub(super) demand_changed: Notify,

    pub(super) cancellation: CancellationToken,

    pub(super) transfer: TransferState,
}

impl DownloadBuffer {
    pub(super) fn new(length: Option<u64>, cancellation: CancellationToken) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(DownloadState {
                chunks: BTreeMap::new(),
                available: RangeSet::new(),
                length,
                requested: None,
                completion: None,
            }),
            changed: Notify::new(),
            demand_changed: Notify::new(),
            cancellation,
            transfer: TransferState::new(length),
        })
    }

    pub(super) fn length(&self) -> Option<u64> {
        self.state.lock().length
    }

    pub(super) fn set_length(&self, length: u64) {
        self.state.lock().length = Some(length);
    }

    /// The producer only requests gaps, so chunks never overlap or change after publication.
    pub(super) fn store_chunk(&self, position: u64, bytes: Bytes) -> io::Result<u64> {
        let length = u64::try_from(bytes.len()).map_err(io::Error::other)?;
        let end = position
            .checked_add(length)
            .ok_or(io::ErrorKind::InvalidData)?;
        if !bytes.is_empty() {
            let mut state = self.state.lock();
            state.available.insert(position..end);
            state.chunks.insert(position, bytes);
            self.publish_progress(&state);
        }
        self.changed.notify_waiters();
        Ok(end)
    }

    fn publish_progress(&self, state: &DownloadState) {
        self.transfer.set_downloaded(
            state
                .available
                .iter()
                .map(|range| range.end - range.start)
                .sum(),
        );
        if let Some(total) = state.length
            && let Some(prefix) = state.available.get(&0)
        {
            self.transfer.set_buffered(BufferedPrefix::Bytes {
                end: prefix.end,
                total,
            });
        }
    }

    pub(super) fn finish(&self, result: Result<(), DownloadError>) {
        let mut state = self.state.lock();
        if result.is_ok() {
            self.publish_progress(&state);
            self.transfer.mark_complete();
        }
        state.completion = Some(result.map_err(Arc::new));
        drop(state);
        self.changed.notify_waiters();
    }

    /// Extend a missing read range through the next hole, retaining useful read-ahead.
    pub(super) fn requested_gap(&self) -> Option<Range<u64>> {
        let state = self.state.lock();
        // Unknown-length responses stay sequential until EOF establishes their extent.
        let length = state.length?;
        let requested = state.requested.as_ref()?;
        let requested = requested.start..requested.end.min(length);
        if requested.is_empty() {
            return None;
        }
        let missing = state.available.gaps(&requested).next()?;
        state.available.gaps(&(missing.start..length)).next()
    }

    pub(super) fn next_gap(&self) -> Option<Range<u64>> {
        self.requested_gap().or_else(|| {
            let state = self.state.lock();
            state
                .available
                .gaps(&(0..state.length?))
                .find(|range| !range.is_empty())
        })
    }

    pub(super) async fn startup(&self, bytes: u64) -> PlaybackResult<()> {
        loop {
            let changed = self.changed.notified();
            pin!(changed);
            changed.as_mut().enable();
            {
                let state = self.state.lock();
                if let Some(result) = &state.completion {
                    return result.clone().map_err(PlaybackError::from);
                }
                let target = state.length.map_or(bytes, |length| bytes.min(length));
                if target == 0
                    || state
                        .available
                        .get(&0)
                        .is_some_and(|range| range.end >= target)
                {
                    return Ok(());
                }
            }
            select! {
                biased;
                () = self.cancellation.cancelled() => return Err(PlaybackError::Cancelled),
                () = changed => {}
            }
        }
    }

    pub(super) async fn completed_chunks(&self) -> PlaybackResult<Vec<Bytes>> {
        loop {
            let changed = self.changed.notified();
            pin!(changed);
            changed.as_mut().enable();
            {
                let state = self.state.lock();
                match &state.completion {
                    Some(Ok(())) => return Ok(state.chunks.values().cloned().collect()),
                    Some(Err(error)) => return Err(error.clone().into()),
                    None => {}
                }
                if self.cancellation.is_cancelled() {
                    return Err(PlaybackError::Cancelled);
                }
            }
            select! {
                biased;
                () = self.cancellation.cancelled() => {}
                () = changed => {}
            }
        }
    }

    /// Return a received chunk, allowing short reads rather than crossing an unfilled gap.
    async fn read_at(
        &self,
        position: u64,
        output: &mut [u8],
        request: &CancellationToken,
    ) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        loop {
            let changed = self.changed.notified();
            pin!(changed);
            changed.as_mut().enable();
            if self.cancellation.is_cancelled() {
                return Err(io::Error::other(PlaybackError::Cancelled));
            }
            if request.is_cancelled() {
                self.state.lock().requested = None;
                return Err(io::Error::other(ReadInterrupted));
            }
            {
                let mut state = self.state.lock();
                if let Some(Err(error)) = &state.completion {
                    return Err(io::Error::other(error.clone()));
                }
                if state.length.is_some_and(|length| position >= length) {
                    return Ok(0);
                }
                let ready =
                    state
                        .chunks
                        .range(..=position)
                        .next_back()
                        .and_then(|(start, bytes)| {
                            let offset = usize::try_from(position - start).ok()?;
                            bytes.get(offset..).filter(|bytes| !bytes.is_empty())
                        });
                if let Some(bytes) = ready {
                    let count = output.len().min(bytes.len());
                    let (destination, _) = output.split_at_mut(count);
                    let (source, _) = bytes.split_at(count);
                    destination.copy_from_slice(source);
                    state.requested = None;
                    return Ok(count);
                }
                let count = u64::try_from(output.len()).map_err(io::Error::other)?;
                let end = position
                    .checked_add(count)
                    .ok_or(io::ErrorKind::InvalidInput)?;
                let range = position..state.length.map_or(end, |length| end.min(length));
                if state.requested.as_ref() != Some(&range) {
                    state.requested = Some(range);
                    self.demand_changed.notify_one();
                }
            }
            select! {
                biased;
                () = self.cancellation.cancelled() => return Err(io::Error::other(PlaybackError::Cancelled)),
                () = request.cancelled() => {
                    self.state.lock().requested = None;
                    return Err(io::Error::other(ReadInterrupted));
                }
                () = changed => {}
            }
        }
    }
}

/// The reader alone owns acquisition's drop guard; capture cannot prolong a stopped track.
pub(super) struct RemoteReader {
    buffer: Arc<DownloadBuffer>,

    position: u64,

    control: ReadControl,

    runtime: Handle,

    _lifetime: DropGuard,
}

impl RemoteReader {
    pub(super) fn new(
        buffer: Arc<DownloadBuffer>,
        control: ReadControl,
        lifetime: DropGuard,
    ) -> Self {
        Self {
            buffer,
            position: 0,
            control,
            runtime: Handle::current(),
            _lifetime: lifetime,
        }
    }
}

impl Read for RemoteReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let request = self.control.request_cancellation();
        // Obsolete decoder work must unwind; Interrupted would make read_exact retry it.
        let count = self
            .runtime
            .block_on(self.buffer.read_at(self.position, output, &request))?;
        self.position = self
            .position
            .checked_add(u64::try_from(count).map_err(io::Error::other)?)
            .ok_or(io::ErrorKind::InvalidInput)?;
        Ok(count)
    }
}

impl Seek for RemoteReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if self.buffer.cancellation.is_cancelled() {
            return Err(io::Error::other(PlaybackError::Cancelled));
        }
        if self.control.request_cancellation().is_cancelled() {
            return Err(io::Error::other(ReadInterrupted));
        }
        let next = match position {
            SeekFrom::Start(position) => Some(position),
            SeekFrom::Current(offset) => self.position.checked_add_signed(offset),
            SeekFrom::End(offset) => self
                .buffer
                .length()
                .ok_or(io::ErrorKind::Unsupported)?
                .checked_add_signed(offset),
        };
        self.position = next.ok_or(io::ErrorKind::InvalidInput)?;
        Ok(self.position)
    }

    fn stream_position(&mut self) -> io::Result<u64> {
        Ok(self.position)
    }
}
