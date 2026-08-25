//! Post-preparation reader tee used to persist exactly the bytes consumed by the decoder.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use color_eyre::eyre::WrapErr;
use mineral_playback::MediaReader;

/// Cloneable completion state for one capture tee.
#[derive(Clone, Default)]
pub(crate) struct CaptureState {
    /// Whether every encoded byte from offset zero has been persisted.
    complete: Arc<AtomicBool>,
}

impl CaptureState {
    /// Returns whether the capture is complete and safe to harvest.
    pub(crate) fn complete(&self) -> bool {
        self.complete.load(Ordering::Acquire)
    }

    /// Marks the capture complete.
    fn mark_complete(&self) {
        self.complete.store(true, Ordering::Release);
    }
}

/// Reader wrapper writing prepared encoded bytes at their logical offsets.
pub(crate) struct CaptureReader {
    /// Decoder-facing prepared media reader.
    inner: Box<dyn MediaReader>,

    /// Persistent capture file, disabled after the first write failure.
    writer: Option<File>,

    /// Current logical reader position.
    position: u64,

    /// Contiguous persisted prefix length starting at offset zero.
    covered_prefix: u64,

    /// Final encoded byte length when known.
    expected_len: Option<u64>,

    /// Completion state observed by the audio snapshot.
    state: CaptureState,
}

impl CaptureReader {
    /// Opens a persistent capture writer before the media reader is consumed.
    ///
    /// # Params:
    ///   - `path`: Persistent capture path.
    pub(crate) fn open_writer(path: &Path) -> color_eyre::Result<File> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .wrap_err_with(|| format!("create capture directory {}", parent.display()))?;
        }
        File::create(path).wrap_err_with(|| format!("create capture file {}", path.display()))
    }

    /// Creates a capture tee from an already-opened writer.
    ///
    /// # Params:
    ///   - `inner`: Prepared encoded media reader.
    ///   - `writer`: Persistent capture file.
    ///   - `expected_len`: Final encoded byte length when known.
    ///
    /// # Return:
    ///   Reader tee and a cloneable completion state.
    pub(crate) fn new(
        inner: Box<dyn MediaReader>,
        writer: File,
        expected_len: Option<u64>,
    ) -> (Self, CaptureState) {
        let state = CaptureState::default();
        (
            Self {
                inner,
                writer: Some(writer),
                position: 0,
                covered_prefix: 0,
                expected_len,
                state: state.clone(),
            },
            state,
        )
    }

    /// Persists one read chunk and advances the contiguous coverage prefix.
    ///
    /// # Params:
    ///   - `start`: Logical offset of the chunk.
    ///   - `bytes`: Encoded bytes returned to the decoder.
    fn capture(&mut self, start: u64, bytes: &[u8]) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        let result = writer
            .seek(SeekFrom::Start(start))
            .and_then(|_| writer.write_all(bytes));
        if let Err(error) = result {
            mineral_log::warn!(
                target: "audio",
                error = mineral_log::chain(&error),
                "capture write failed; disabling capture"
            );
            self.writer = None;
            return;
        }
        if start <= self.covered_prefix {
            let Ok(chunk_len) = u64::try_from(bytes.len()) else {
                mineral_log::warn!(target: "audio", "capture chunk length exceeds u64");
                self.writer = None;
                return;
            };
            self.covered_prefix = self.covered_prefix.max(start.saturating_add(chunk_len));
        }
        if self
            .expected_len
            .is_some_and(|expected| self.covered_prefix >= expected)
        {
            self.state.mark_complete();
        }
    }
}

impl Read for CaptureReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let start = self.position;
        let read = self.inner.read(buffer)?;
        if read == 0 {
            if self.expected_len.is_none() && start == self.covered_prefix {
                self.state.mark_complete();
            }
            return Ok(0);
        }
        let bytes = buffer
            .get(..read)
            .ok_or_else(|| io::Error::other("reader returned an invalid byte count"))?;
        self.capture(start, bytes);
        let read_u64 = u64::try_from(read).map_err(io::Error::other)?;
        self.position = start
            .checked_add(read_u64)
            .ok_or_else(|| io::Error::other("capture reader position overflow"))?;
        Ok(read)
    }
}

impl Seek for CaptureReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.position = self.inner.seek(position)?;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read};

    use super::CaptureReader;

    /// Capture persists exactly the prepared bytes returned to the decoder.
    #[test]
    fn captures_prepared_reader_output() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("capture.part");
        let writer = CaptureReader::open_writer(&path)?;
        let (mut reader, state) = CaptureReader::new(
            Box::new(Cursor::new(b"decoder-ready".to_vec())),
            writer,
            Some(13),
        );
        let mut output = Vec::new();
        reader.read_to_end(&mut output)?;
        drop(reader);
        assert_eq!(output, b"decoder-ready");
        assert_eq!(std::fs::read(path)?, b"decoder-ready");
        assert!(state.complete());
        Ok(())
    }
}
