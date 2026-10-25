use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use mineral_playback::ReadControl;

use super::error::Error;
use super::store::MediaStore;

pub(super) struct MediaReader {
    store: Arc<MediaStore>,

    position: u64,

    local: Option<(usize, File)>,

    read_control: Option<ReadControl>,
}

impl MediaReader {
    pub(super) fn new(store: Arc<MediaStore>, read_control: Option<ReadControl>) -> Self {
        Self {
            store,
            position: 0,
            local: None,
            read_control,
        }
    }
}

impl Read for MediaReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.store.cancellation.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                Error::Cancelled,
            ));
        }
        if output.is_empty() || self.position >= self.store.byte_len {
            return Ok(0);
        }
        if let Some(metadata) = self
            .store
            .metadata
            .iter()
            .find(|span| span.range.contains(&self.position))
        {
            let offset =
                usize::try_from(self.position - metadata.range.start).map_err(io::Error::other)?;
            let bytes = metadata.bytes.get(offset..).ok_or_else(|| {
                io::Error::other(Error::InvalidMedia {
                    field: "metadata read offset",
                })
            })?;
            let count = copy_available(output, bytes)?;
            self.position += u64::try_from(count).map_err(io::Error::other)?;
            return Ok(count);
        }
        let index = self
            .store
            .segments
            .partition_point(|segment| segment.range.end <= self.position);
        let segment = self.store.segments.get(index).ok_or_else(|| {
            io::Error::other(Error::InvalidMedia {
                field: "sample read range",
            })
        })?;
        if self
            .local
            .as_ref()
            .is_none_or(|(current, _)| *current != index)
        {
            let request = self
                .read_control
                .as_ref()
                .map(ReadControl::request_cancellation)
                .unwrap_or_else(|| self.store.cancellation.clone());
            let path = self
                .store
                .runtime
                .block_on(self.store.segment(index, request))
                .map_err(io::Error::other)?;
            self.local = Some((index, File::open(path)?));
        }
        let Some((_, file)) = &mut self.local else {
            return Err(io::Error::other(Error::InvalidMedia {
                field: "opened segment",
            }));
        };
        file.seek(SeekFrom::Start(self.position - segment.range.start))?;
        let count = file.read(output)?;
        self.position += u64::try_from(count).map_err(io::Error::other)?;
        Ok(count)
    }
}

impl Seek for MediaReader {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        self.position = match target {
            SeekFrom::Start(position) => position,
            SeekFrom::Current(delta) => {
                self.position.checked_add_signed(delta).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "negative media seek")
                })?
            }
            SeekFrom::End(delta) => {
                self.store
                    .byte_len
                    .checked_add_signed(delta)
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "negative media seek")
                    })?
            }
        };
        Ok(self.position)
    }
}

fn copy_available(output: &mut [u8], input: &[u8]) -> io::Result<usize> {
    let count = output.len().min(input.len());
    let destination = output.get_mut(..count).ok_or_else(|| {
        io::Error::other(Error::InvalidMedia {
            field: "read destination",
        })
    })?;
    let source = input.get(..count).ok_or_else(|| {
        io::Error::other(Error::InvalidMedia {
            field: "read source",
        })
    })?;
    destination.copy_from_slice(source);
    Ok(count)
}
