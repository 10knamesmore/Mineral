use std::io::{self, Read, Seek, SeekFrom};

use symphonia::core::io::MediaSource;

use super::DecoderError;

pub(super) struct EncodedReader<R> {
    reader: R,

    byte_len: u64,
}

impl<R: Read + Seek> EncodedReader<R> {
    pub(super) fn new(mut reader: R) -> Result<Self, DecoderError> {
        let position = reader
            .stream_position()
            .map_err(DecoderError::ReaderPosition)?;
        let byte_len = reader
            .seek(SeekFrom::End(0))
            .map_err(DecoderError::ReaderLength)?;
        reader
            .seek(SeekFrom::Start(position))
            .map_err(DecoderError::RestoreReader)?;
        Ok(Self { reader, byte_len })
    }
}

impl<R: Read> Read for EncodedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.reader.read(buffer)
    }
}

impl<R: Seek> Seek for EncodedReader<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.reader.seek(position)
    }
}

impl<R: Read + Seek + Send + Sync> MediaSource for EncodedReader<R> {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.byte_len)
    }
}
