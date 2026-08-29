//! Persistent stream-download storage for producer-owned media capture.

use std::fs::File;
use std::io::{self, BufReader};
use std::path::PathBuf;

use stream_download::storage::StorageProvider;

/// Writes producer bytes to a caller-selected file that remains after the reader is dropped.
pub(super) struct CaptureStorageProvider {
    /// Unique temporary capture path.
    path: PathBuf,
}

impl CaptureStorageProvider {
    /// Creates persistent storage for one capture attempt.
    ///
    /// # Params:
    ///   - `path`: Unique temporary path for the producer output.
    pub(super) fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl StorageProvider for CaptureStorageProvider {
    type Reader = BufReader<File>;
    type Writer = File;

    fn into_reader_writer(
        self,
        _content_length: Option<u64>,
    ) -> io::Result<(Self::Reader, Self::Writer)> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let writer = File::create(&self.path)?;
        let reader = BufReader::new(File::open(&self.path)?);
        Ok((reader, writer))
    }
}
