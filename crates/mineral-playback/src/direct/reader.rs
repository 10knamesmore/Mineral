//! Encoded-media reads and seeks scoped to a playback instance.

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
