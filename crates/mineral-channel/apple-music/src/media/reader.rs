use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use mineral_playback::ReadControl;
use tokio_util::sync::DropGuard;

use super::error::Error;
use super::store::MediaStore;

/// Complete encoded-file access, deliberately separate from progressive audio packets.
pub(super) struct MediaReader {
    store: Arc<MediaStore>,
    file: Option<File>,
    read_control: Option<ReadControl>,
    _lifetime: Arc<DropGuard>,
}

impl MediaReader {
    pub(super) fn new(
        store: Arc<MediaStore>,
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
                .block_on(self.store.export(request.clone()))
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

impl Read for MediaReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        self.file()?.read(output)
    }
}

impl Seek for MediaReader {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        self.file()?.seek(target)
    }
}
