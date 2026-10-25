use std::io::{self, Write};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use mineral_playback::{CaptureReceipt, CaptureTarget, CapturedMedia, ReadControl, TransferState};
use reqwest::Client;
use tempfile::{NamedTempFile, TempDir};
use tokio::runtime::Handle;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use super::cenc::AudioInitialization;
use super::error::{Error, Result};
use super::index::{AudioIndex, MetadataRange};
use super::reader::MediaReader;
use super::transport::{Part, download};

pub(super) struct Segment {
    pub range: Range<u64>,

    source: Part,

    file: OnceCell<PathBuf>,
}

pub(super) struct MediaStore {
    pub metadata: Vec<MetadataRange>,

    pub segments: Vec<Segment>,

    pub byte_len: u64,

    pub cancellation: CancellationToken,

    pub runtime: Handle,

    pub transfer: TransferState,

    directory: TempDir,

    initialization: AudioInitialization,

    key: Zeroizing<[u8; 16]>,

    client: Client,
}

impl MediaStore {
    pub(super) async fn new(
        index: AudioIndex,
        initialization: AudioInitialization,
        key: Zeroizing<[u8; 16]>,
        client: Client,
        cancellation: CancellationToken,
    ) -> Result<Arc<Self>> {
        let directory = tokio::task::spawn_blocking(tempfile::tempdir).await??;
        let transfer = TransferState::new(Some(index.byte_len));
        for metadata in &index.metadata {
            transfer.add_downloaded(u64::try_from(metadata.bytes.len())?);
        }
        Ok(Arc::new(Self {
            metadata: index.metadata,
            segments: index
                .segments
                .into_iter()
                .map(|segment| Segment {
                    range: segment.range,
                    source: segment.source,
                    file: OnceCell::new(),
                })
                .collect(),
            byte_len: index.byte_len,
            cancellation,
            runtime: Handle::current(),
            transfer,
            directory,
            initialization,
            key,
            client,
        }))
    }

    pub(super) fn reader(self: &Arc<Self>, read_control: Option<ReadControl>) -> MediaReader {
        MediaReader::new(self.clone(), read_control)
    }

    pub(super) async fn segment(
        &self,
        index: usize,
        request: CancellationToken,
    ) -> Result<PathBuf> {
        let segment = self.segments.get(index).ok_or(Error::InvalidMedia {
            field: "segment index",
        })?;
        tokio::select! {
            biased;
            () = request.cancelled() => Err(Error::Cancelled),
            result = segment.file.get_or_try_init(|| self.prepare_segment(index, segment)) => result.cloned(),
        }
    }

    async fn prepare_segment(&self, index: usize, segment: &Segment) -> Result<PathBuf> {
        let bytes = download(&self.client, &segment.source).await?;
        let clear = self.initialization.decrypt_fragment_at(
            &bytes,
            segment.source.resource_offset()?,
            &self.key,
            self.initialization.key_id(),
        )?;
        if u64::try_from(clear.len())? != segment.range.end - segment.range.start {
            return Err(Error::InvalidMedia {
                field: "prepared segment length",
            });
        }
        let path = self
            .directory
            .path()
            .join(format!("segment-{index}.samples"));
        // This future is driven by a decoder/capture blocking thread. Publish atomically,
        // so cancellation cannot leave a later reader observing a partially written segment.
        let mut temporary = NamedTempFile::new_in(self.directory.path())?;
        temporary.write_all(&clear)?;
        temporary
            .persist(&path)
            .map_err(|error| Error::Io(error.error))?;
        self.transfer.add_downloaded(u64::try_from(clear.len())?);
        if self.transfer.snapshot().downloaded == self.byte_len {
            self.transfer.mark_complete();
        }
        mineral_log::debug!(target: "apple_music", segment = index, bytes = clear.len(), "media segment prepared");
        Ok(path)
    }

    /// Cache production uses its own cursor and is not interrupted by playback seeks.
    pub(super) fn capture(self: &Arc<Self>, target: &CaptureTarget) -> CaptureReceipt {
        let store = self.clone();
        let destination = target.path().to_path_buf();
        let worker = tokio::task::spawn_blocking(move || store.capture_file(destination));
        CaptureReceipt::new(async move { worker.await.map_err(Error::from)?.map_err(Into::into) })
    }

    fn capture_file(self: Arc<Self>, destination: PathBuf) -> Result<CapturedMedia> {
        let parent = destination.parent().ok_or(Error::InvalidMedia {
            field: "capture directory",
        })?;
        std::fs::create_dir_all(parent)?;
        let mut temporary = NamedTempFile::new_in(parent)?;
        let mut reader = self.reader(None);
        let bytes = io::copy(&mut reader, temporary.as_file_mut())?;
        if bytes != self.byte_len {
            return Err(Error::InvalidMedia {
                field: "complete capture length",
            });
        }
        temporary.as_file().sync_all()?;
        temporary
            .persist(&destination)
            .map_err(|error| Error::Io(error.error))?;
        Ok(CapturedMedia::new(destination, bytes))
    }
}
