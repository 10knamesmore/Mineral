//! Provides lazy Read/Seek access to a complete MP4 assembled in memory.

use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::sync::Arc;

use mineral_playback::ReadControl;
use tokio_util::sync::DropGuard;

use super::super::error::Error;
use super::export;
use super::segments::SegmentStore;

/// Complete encoded-file access, deliberately separate from progressive audio packets.
/// The first read or seek waits for shared in-memory assembly; the reader keeps the playback
/// root alive even while no packet consumer is reading.
pub(crate) struct CompleteFileReader {
    store: Arc<SegmentStore>,

    cursor: Option<Cursor<Arc<[u8]>>>,

    read_control: ReadControl,

    _lifetime: Arc<DropGuard>,
}

impl CompleteFileReader {
    pub(crate) fn new(
        store: Arc<SegmentStore>,
        read_control: ReadControl,
        lifetime: Arc<DropGuard>,
    ) -> Self {
        Self {
            store,
            cursor: None,
            read_control,
            _lifetime: lifetime,
        }
    }

    fn cursor(&mut self) -> io::Result<&mut Cursor<Arc<[u8]>>> {
        let request = self.read_control.request_cancellation();
        if self.store.cancellation.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                Error::Cancelled,
            ));
        }
        if request.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, Error::Cancelled));
        }
        if let Some(ref mut cursor) = self.cursor {
            Ok(cursor)
        } else {
            let bytes = self
                .store
                .runtime
                .block_on(export::complete_bytes(&self.store, request.clone()))
                .map_err(|error| {
                    let kind = if self.store.cancellation.is_cancelled() {
                        io::ErrorKind::ConnectionAborted
                    } else if request.is_cancelled() {
                        io::ErrorKind::Interrupted
                    } else {
                        io::ErrorKind::Other
                    };
                    io::Error::new(kind, error)
                })?;
            Ok(self.cursor.insert(Cursor::new(bytes)))
        }
    }
}

impl Read for CompleteFileReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        self.cursor()?.read(output)
    }
}

impl Seek for CompleteFileReader {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        self.cursor()?.seek(target)
    }
}
