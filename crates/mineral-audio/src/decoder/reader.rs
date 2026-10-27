use std::io::{self, Read, Seek, SeekFrom};

use symphonia::core::io::MediaSource;

pub(super) struct EncodedReader<R> {
    reader: R,

    byte_len: Option<u64>,
}

impl<R> EncodedReader<R> {
    /// A streaming reader's seek may wait for acquisition. Length inspection must not seek.
    pub(super) fn new(reader: R, byte_len: Option<u64>) -> Self {
        Self { reader, byte_len }
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
        self.byte_len
    }
}
