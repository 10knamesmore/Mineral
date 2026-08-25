//! Reader wrappers enforcing cancellation and forward-only access.

use std::io::{self, Read, Seek, SeekFrom};

use tokio_util::sync::CancellationToken;

use crate::MediaReader;

/// Rejects reads and seeks after playback instance cancellation.
pub(super) struct CancellationReader {
    /// Wrapped encoded-media reader.
    inner: Box<dyn MediaReader>,

    /// Playback instance cancellation token.
    cancellation: CancellationToken,
}

impl CancellationReader {
    /// Wraps a reader with cancellation checks.
    ///
    /// # Params:
    ///   - `inner`: Encoded-media reader.
    ///   - `cancellation`: Playback instance cancellation token.
    pub(super) fn new(inner: Box<dyn MediaReader>, cancellation: CancellationToken) -> Self {
        Self {
            inner,
            cancellation,
        }
    }

    /// Returns an interrupted IO error after cancellation.
    fn cancelled_error() -> io::Error {
        io::Error::new(io::ErrorKind::Interrupted, "playback instance cancelled")
    }
}

impl Read for CancellationReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancellation.is_cancelled() {
            return Err(Self::cancelled_error());
        }
        self.inner.read(buf)
    }
}

impl Seek for CancellationReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if self.cancellation.is_cancelled() {
            return Err(Self::cancelled_error());
        }
        self.inner.seek(position)
    }
}

/// Provides the `Seek` facade while rejecting backward and end-relative access.
pub(super) struct ForwardOnlyReader {
    /// Wrapped streaming reader.
    inner: Box<dyn MediaReader>,

    /// Logical encoded byte position.
    position: u64,
}

impl ForwardOnlyReader {
    /// Wraps a reader at position zero.
    ///
    /// # Params:
    ///   - `inner`: Streaming reader to constrain.
    pub(super) fn new(inner: Box<dyn MediaReader>) -> Self {
        Self { inner, position: 0 }
    }

    /// Resolves an allowed seek target.
    ///
    /// # Params:
    ///   - `position`: Requested seek operation.
    ///
    /// # Return:
    ///   A target at or after the current position.
    fn target(&self, position: SeekFrom) -> io::Result<u64> {
        let target = match position {
            SeekFrom::Start(target) => target,
            SeekFrom::Current(delta) if delta >= 0 => {
                let forward = u64::try_from(delta).map_err(io::Error::other)?;
                self.position.checked_add(forward).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "forward seek overflow")
                })?
            }
            SeekFrom::Current(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "forward-only media rejects backward seek",
                ));
            }
            SeekFrom::End(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "forward-only media has no end-relative seek",
                ));
            }
        };
        if target < self.position {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "forward-only media rejects backward seek",
            ));
        }
        Ok(target)
    }
}

impl Read for ForwardOnlyReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buf)?;
        let read_u64 = u64::try_from(read).map_err(io::Error::other)?;
        self.position = self
            .position
            .checked_add(read_u64)
            .ok_or_else(|| io::Error::other("reader position overflow"))?;
        Ok(read)
    }
}

impl Seek for ForwardOnlyReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let target = self.target(position)?;
        let remaining = target.saturating_sub(self.position);
        if remaining == 0 {
            return Ok(self.position);
        }
        let copied = io::copy(
            &mut Read::by_ref(&mut self.inner).take(remaining),
            &mut io::sink(),
        )?;
        self.position = self.position.saturating_add(copied);
        if copied != remaining {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "forward seek reached end of media",
            ));
        }
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Seek, SeekFrom};

    use super::ForwardOnlyReader;

    /// Forward-only readers may skip ahead but reject backward access.
    #[test]
    fn forward_only_reader_rejects_backward_seek() -> color_eyre::Result<()> {
        let mut reader = ForwardOnlyReader::new(Box::new(Cursor::new(b"abcdef".to_vec())));
        assert_eq!(reader.seek(SeekFrom::Start(3))?, 3);
        let mut byte = [0u8; 1];
        assert_eq!(reader.read(&mut byte)?, 1);
        assert_eq!(byte, [b'd']);
        assert!(reader.seek(SeekFrom::Start(2)).is_err());
        Ok(())
    }
}
