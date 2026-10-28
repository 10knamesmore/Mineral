use std::fs::File;
use std::io::{self, Seek, SeekFrom};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures_util::future::{BoxFuture, Shared};
use futures_util::{FutureExt, StreamExt, stream};
use mineral_playback::{
    BufferedPrefix, CaptureReceipt, CaptureTarget, CapturedMedia, ReadControl, TransferState,
};
use parking_lot::Mutex;
use reqwest::Client;
use tempfile::{NamedTempFile, TempDir};
use tokio::runtime::Handle;
use tokio::sync::{OnceCell, Semaphore};
use tokio_util::sync::{CancellationToken, DropGuard};
use zeroize::Zeroizing;

use super::cenc::AudioInitialization;
use super::error::{Error, Result};
use super::index::{self, Fragment, RegularMp4};
use super::reader::MediaReader;
use super::transport::{MediaSegment, Part, download};

type SegmentPreparation =
    Shared<BoxFuture<'static, std::result::Result<Arc<PreparedSegment>, Arc<Error>>>>;

struct Segment {
    source: Part,

    header: OnceCell<std::result::Result<Arc<Fragment>, Arc<Error>>>,

    prepared: OnceLock<SegmentPreparation>,
}

pub(super) struct PreparedSegment {
    pub path: PathBuf,

    /// Exact decode interval in initialization timescale ticks.
    pub timeline: Range<u64>,

    fragment: Arc<Fragment>,

    payload_offset: u64,
}

struct Progress {
    /// Exact intervals learned from prepared fragments or lazily fetched headers.
    timelines: Vec<Option<Range<u64>>>,

    /// Only complete, decrypted and atomically published segments are playable.
    prepared: Vec<bool>,
}

pub(super) struct MediaStore {
    pub cancellation: CancellationToken,

    pub runtime: Handle,

    pub transfer: TransferState,

    pub declared_duration: Duration,

    pub timescale: u32,

    segments: Vec<Segment>,

    estimated_starts: Vec<Duration>,

    progress: Mutex<Progress>,

    export: OnceCell<std::result::Result<PathBuf, Arc<Error>>>,

    directory: TempDir,

    initialization: AudioInitialization,

    key: Zeroizing<[u8; 16]>,

    client: Client,

    header_prefetch: NonZeroUsize,

    concurrency: NonZeroUsize,

    preparation_slots: Semaphore,
}

impl MediaStore {
    pub(super) async fn new(
        segments: Vec<MediaSegment>,
        initialization: AudioInitialization,
        key: Zeroizing<[u8; 16]>,
        client: Client,
        cancellation: CancellationToken,
        header_prefetch: NonZeroUsize,
        concurrency: NonZeroUsize,
    ) -> Result<Arc<Self>> {
        let directory = tokio::task::spawn_blocking(tempfile::tempdir).await??;
        let mut declared_duration = Duration::ZERO;
        let mut estimated_starts = Vec::new();
        for segment in &segments {
            estimated_starts.push(declared_duration);
            declared_duration = declared_duration
                .checked_add(segment.duration)
                .ok_or(Error::Overflow)?;
        }
        let count = segments.len();
        let store = Arc::new(Self {
            timescale: initialization.timescale().get(),
            declared_duration,
            estimated_starts,
            progress: Mutex::new(Progress {
                timelines: vec![None; count],
                prepared: vec![false; count],
            }),
            segments: segments
                .into_iter()
                .map(|segment| Segment {
                    source: segment.source,
                    header: OnceCell::new(),
                    prepared: OnceLock::new(),
                })
                .collect(),
            // Count unique clear AAC payload bytes until the complete encoded file
            // supplies its actual length, independently of time buffering.
            transfer: TransferState::new(None),
            cancellation,
            runtime: Handle::current(),
            export: OnceCell::new(),
            directory,
            initialization,
            key,
            client,
            header_prefetch,
            concurrency,
            preparation_slots: Semaphore::new(concurrency.get()),
        });
        store.publish_progress(&store.progress.lock());
        Ok(store)
    }

    pub(super) fn reader(
        self: &Arc<Self>,
        control: Option<ReadControl>,
        lifetime: Arc<DropGuard>,
    ) -> MediaReader {
        MediaReader::new(self.clone(), control, lifetime)
    }

    pub(super) fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub(super) fn exact_duration_ticks(&self) -> Option<u64> {
        self.progress
            .lock()
            .timelines
            .last()
            .and_then(Option::as_ref)
            .map(|timeline| timeline.end)
    }

    pub(super) fn duration(&self, ticks: u64) -> Duration {
        let scale = u64::from(self.timescale);
        Duration::from_secs(ticks / scale)
            + Duration::from_nanos((ticks % scale) * 1_000_000_000 / scale)
    }

    pub(super) fn estimated_segment(&self, ticks: u64) -> usize {
        let time = self.duration(ticks);
        self.estimated_starts
            .partition_point(|start| *start <= time)
            .saturating_sub(1)
    }

    /// The producer owns acquisition independently of decoder reads and optional capture.
    pub(super) fn acquire(self: &Arc<Self>) {
        let store = self.clone();
        self.runtime.spawn(async move {
            // Queue only a bounded window: a seek can acquire the next released
            // preparation slot instead of waiting behind the rest of the song.
            let mut acquisition = stream::iter(0..store.segments.len())
                .map(|index| {
                    let store = store.clone();
                    async move { store.segment(index, store.cancellation.clone()).await }
                })
                .buffer_unordered(store.concurrency.get());
            while let Some(result) = acquisition.next().await {
                if let Err(error) = result {
                    if !store.cancellation.is_cancelled() {
                        mineral_log::error!(target: "apple_music", error = %error, "background segment preparation failed");
                    }
                    return;
                }
            }
        });
    }

    pub(super) async fn startup(self: &Arc<Self>, target: Duration) -> Result<()> {
        let estimated_count = self
            .estimated_starts
            .partition_point(|start| *start < target);
        let initial = stream::iter(0..estimated_count)
            .map(|index| self.segment(index, self.cancellation.clone()))
            .buffered(self.concurrency.get());
        let remaining = stream::iter(estimated_count..self.segments.len())
            .then(|index| self.segment(index, self.cancellation.clone()));
        let mut startup = initial.chain(remaining).boxed();
        // EXTINF plans the initial window; actual sample durations decide readiness.
        while let Some(segment) = startup.next().await {
            let segment = segment?;
            if self.duration(segment.timeline.end) >= target {
                return Ok(());
            }
        }
        Ok(())
    }

    pub(super) async fn segment(
        self: &Arc<Self>,
        index: usize,
        request: CancellationToken,
    ) -> Result<Arc<PreparedSegment>> {
        let segment = self.segments.get(index).ok_or(Error::InvalidMedia {
            field: "segment index",
        })?;
        tokio::select! {
            biased;
            () = request.cancelled() => Err(Error::Cancelled),
            () = self.cancellation.cancelled() => Err(Error::Cancelled),
            result = async {
                // A cancelled seek stops waiting, while the root-owned preparation
                // continues for background acquisition and other consumers.
                let preparation = segment.prepared.get_or_init(|| {
                    let store = self.clone();
                    let worker = self.runtime.spawn(async move {
                        tokio::select! {
                            biased;
                            () = store.cancellation.cancelled() => Err(Error::Cancelled),
                            result = async {
                                let _slot = store.preparation_slots.acquire().await?;
                                store.prepare_segment(index).await
                            } => result,
                        }
                    });
                    async move {
                        match worker.await {
                            Ok(result) => result.map(Arc::new).map_err(Arc::new),
                            Err(error) => Err(Arc::new(Error::from(error))),
                        }
                    }.boxed().shared()
                });
                preparation.clone().await
            } => result.map_err(Error::Preparation),
        }
    }

    pub(super) async fn locate(&self, ticks: u64) -> Result<usize> {
        let mut index = self.estimated_segment(ticks);
        loop {
            let header = self.header(index).await?;
            let timeline = self.timeline(index, &header).await?;
            if ticks < timeline.start {
                index = index.checked_sub(1).ok_or(Error::InvalidMedia {
                    field: "seek before track",
                })?;
            } else if ticks >= timeline.end {
                if index + 1 == self.segments.len() {
                    return Ok(index);
                }
                index += 1;
            } else {
                return Ok(index);
            }
        }
    }

    async fn header(&self, index: usize) -> Result<Arc<Fragment>> {
        let segment = self.segments.get(index).ok_or(Error::InvalidMedia {
            field: "segment index",
        })?;
        tokio::select! {
            biased;
            () = self.cancellation.cancelled() => Err(Error::Cancelled),
            result = segment.header.get_or_init(|| async {
                let result = async {
                    let header = index::fragment_header(&self.client, &segment.source, self.header_prefetch).await?;
                    Fragment::parse(&header, &self.initialization)
                }.await;
                result.map(Arc::new).map_err(Arc::new)
            }) => result.clone().map_err(Error::Preparation),
        }
    }

    /// Missing tfdt requires only preceding headers up to a known exact origin.
    async fn timeline(&self, index: usize, fragment: &Fragment) -> Result<Range<u64>> {
        let mut unresolved = Vec::new();
        let mut current = index;
        let mut start = fragment.decode_start;
        while start.is_none() {
            if current == 0 {
                start = Some(0);
                break;
            }
            current -= 1;
            let known = self
                .progress
                .lock()
                .timelines
                .get(current)
                .ok_or(Error::InvalidMedia {
                    field: "segment timeline index",
                })?
                .clone();
            if let Some(known) = known {
                start = Some(known.end);
                break;
            }
            let preceding = self.header(current).await?;
            start = preceding.decode_start;
            unresolved.push((current, preceding));
        }
        let mut start = start.ok_or(Error::InvalidMedia {
            field: "fragment decode origin",
        })?;
        let mut progress = self.progress.lock();
        for (position, preceding) in unresolved.into_iter().rev() {
            let end = start
                .checked_add(preceding.duration)
                .ok_or(Error::Overflow)?;
            Self::record_timeline(&mut progress, position, start..end)?;
            start = end;
        }
        let end = start
            .checked_add(fragment.duration)
            .ok_or(Error::Overflow)?;
        Self::record_timeline(&mut progress, index, start..end)?;
        Ok(start..end)
    }

    fn record_timeline(progress: &mut Progress, index: usize, range: Range<u64>) -> Result<()> {
        let previous_end = index
            .checked_sub(1)
            .and_then(|previous| progress.timelines.get(previous))
            .and_then(Option::as_ref)
            .map(|previous| previous.end);
        let next_start = progress
            .timelines
            .get(index + 1)
            .and_then(Option::as_ref)
            .map(|next| next.start);
        let current = progress
            .timelines
            .get_mut(index)
            .ok_or(Error::InvalidMedia {
                field: "segment timeline index",
            })?;
        if (index == 0 && range.start != 0)
            || previous_end.is_some_and(|end| end != range.start)
            || next_start.is_some_and(|start| start != range.end)
            || current.as_ref().is_some_and(|known| *known != range)
        {
            return Err(Error::Unsupported {
                layout: "non-contiguous decode timeline",
            });
        }
        *current = Some(range);
        Ok(())
    }

    async fn prepare_segment(self: &Arc<Self>, index: usize) -> Result<PreparedSegment> {
        let segment = self.segments.get(index).ok_or(Error::InvalidMedia {
            field: "segment index",
        })?;
        let bytes = download(&self.client, &segment.source).await?;
        let fragment = segment
            .header
            .get_or_init(|| async {
                let result = index::fragment_from_bytes(&bytes)
                    .and_then(|header| Fragment::parse(&header, &self.initialization));
                result.map(Arc::new).map_err(Arc::new)
            })
            .await
            .clone()
            .map_err(Error::Preparation)?;
        let timeline = self.timeline(index, &fragment).await?;
        let resource_offset = segment.source.resource_offset()?;
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.write_segment(index, &bytes, resource_offset, fragment, timeline)
        })
        .await?
    }

    /// Decryption, remuxing and file publication stay off the async I/O workers.
    fn write_segment(
        &self,
        index: usize,
        bytes: &[u8],
        resource_offset: u64,
        fragment: Arc<Fragment>,
        timeline: Range<u64>,
    ) -> Result<PreparedSegment> {
        if self.cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let clear = self
            .initialization
            .decrypt_fragment_at(bytes, resource_offset, &self.key)?;
        if u64::try_from(clear.len())? != fragment.byte_len {
            return Err(Error::InvalidMedia {
                field: "decrypted sample length",
            });
        }
        let mut temporary = NamedTempFile::new_in(self.directory.path())?;
        let mut muxer = RegularMp4::new(temporary.as_file_mut(), &self.initialization)?;
        let payload_offset = muxer.position();
        muxer.append(temporary.as_file_mut(), &fragment, &mut clear.as_slice())?;
        muxer.finish(temporary.as_file_mut())?;
        let path = self.directory.path().join(format!("segment-{index}.mp4"));
        temporary
            .persist(&path)
            .map_err(|error| Error::Io(error.error))?;
        self.transfer.add_downloaded(fragment.byte_len);
        let mut progress = self.progress.lock();
        *progress
            .prepared
            .get_mut(index)
            .ok_or(Error::InvalidMedia {
                field: "prepared segment index",
            })? = true;
        self.publish_progress(&progress);
        Ok(PreparedSegment {
            path,
            timeline,
            fragment,
            payload_offset,
        })
    }

    fn publish_progress(&self, progress: &Progress) {
        let count = progress.prepared.iter().take_while(|ready| **ready).count();
        let end = progress
            .timelines
            .iter()
            .take(count)
            .next_back()
            .and_then(Option::as_ref)
            .map_or(Duration::ZERO, |timeline| self.duration(timeline.end));
        // EXTINF declares the duration until all exact sample timings are known.
        let total = if count == self.segments.len() {
            end
        } else {
            self.declared_duration.max(end)
        };
        self.transfer
            .set_buffered(BufferedPrefix::Time { end, total });
        if count == self.segments.len() {
            self.transfer.mark_complete();
        }
    }

    pub(super) async fn export(self: &Arc<Self>, request: CancellationToken) -> Result<PathBuf> {
        tokio::select! {
            biased;
            () = request.cancelled() => Err(Error::Cancelled),
            () = self.cancellation.cancelled() => Err(Error::Cancelled),
            result = self.export.get_or_init(|| async {
                self.build_export().await.map_err(Arc::new)
            }) => result.clone().map_err(Error::Preparation),
        }
    }

    async fn build_export(self: &Arc<Self>) -> Result<PathBuf> {
        let mut temporary = NamedTempFile::new_in(self.directory.path())?;
        let mut muxer = RegularMp4::new(temporary.as_file_mut(), &self.initialization)?;
        for index in 0..self.segments.len() {
            let prepared = self.segment(index, self.cancellation.clone()).await?;
            let mut input = File::open(&prepared.path)?;
            input.seek(SeekFrom::Start(prepared.payload_offset))?;
            muxer.append(temporary.as_file_mut(), &prepared.fragment, &mut input)?;
        }
        let byte_len = muxer.finish(temporary.as_file_mut())?;
        let path = self.directory.path().join("complete.mp4");
        temporary
            .persist(&path)
            .map_err(|error| Error::Io(error.error))?;
        self.transfer.set_downloaded(byte_len);
        Ok(path)
    }

    pub(super) fn capture(
        self: &Arc<Self>,
        target: &CaptureTarget,
        lifetime: Arc<DropGuard>,
    ) -> CaptureReceipt {
        let store = self.clone();
        let destination = target.path().to_path_buf();
        let worker = tokio::task::spawn_blocking(move || store.capture_file(destination, lifetime));
        CaptureReceipt::new(async move { worker.await.map_err(Error::from)?.map_err(Into::into) })
    }

    fn capture_file(
        self: Arc<Self>,
        destination: PathBuf,
        lifetime: Arc<DropGuard>,
    ) -> Result<CapturedMedia> {
        let parent = destination.parent().ok_or(Error::InvalidMedia {
            field: "capture directory",
        })?;
        std::fs::create_dir_all(parent)?;
        let mut temporary = NamedTempFile::new_in(parent)?;
        let mut reader = self.reader(None, lifetime);
        let bytes = io::copy(&mut reader, temporary.as_file_mut())?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&destination)
            .map_err(|error| Error::Io(error.error))?;
        Ok(CapturedMedia::new(destination, bytes))
    }
}
