use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use mineral_playback::ReadControl;
use tokio_util::sync::DropGuard;

use super::super::error::Error;
use super::export;
use super::segments::SegmentStore;

/// Complete encoded-file access, deliberately separate from progressive audio packets.
/// The first read or seek waits for the shared export; the reader keeps the playback
/// root alive even while no packet consumer is reading.
pub(crate) struct CompleteFileReader {
    store: Arc<SegmentStore>,

    file: Option<File>,

    read_control: Option<ReadControl>,

    _lifetime: Arc<DropGuard>,
}

impl CompleteFileReader {
    pub(crate) fn new(
        store: Arc<SegmentStore>,
        read_control: Option<ReadControl>,
        lifetime: Arc<DropGuard>,
    ) -> Self {
        Self {
            store,
            file: None,
            read_control,
            _lifetime: lifetime,
        }
    }

    fn file(&mut self) -> io::Result<&mut File> {
        let request = self
            .read_control
            .as_ref()
            .map(ReadControl::request_cancellation)
            .unwrap_or_else(|| self.store.cancellation.clone());
        if self.store.cancellation.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                Error::Cancelled,
            ));
        }
        if request.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, Error::Cancelled));
        }
        if self.file.is_none() {
            let path = self
                .store
                .runtime
                .block_on(export::complete_file(&self.store, request.clone()))
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
            self.file = Some(File::open(path)?);
        }
        self.file.as_mut().ok_or_else(|| {
            io::Error::other(Error::InvalidMedia {
                field: "complete export file",
            })
        })
    }
}

impl Read for CompleteFileReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        self.file()?.read(output)
    }
}

impl Seek for CompleteFileReader {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        self.file()?.seek(target)
    }
}
