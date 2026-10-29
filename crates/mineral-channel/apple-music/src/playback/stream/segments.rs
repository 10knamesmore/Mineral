//! Deduplicates header and clear-segment preparation under one playback cancellation root.

use std::io::Cursor;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures_util::future::{BoxFuture, Shared};
use futures_util::{FutureExt, StreamExt, stream};
use mineral_playback::{BufferedPrefix, TransferState};
use parking_lot::Mutex;
use reqwest::Client;
use tokio::runtime::Handle;
use tokio::sync::{OnceCell, Semaphore};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use super::super::cdn::{self, MediaResource, download};
use super::super::cenc::AudioInitialization;
use super::super::error::{Error, Result};
use super::super::hls::MediaSegment;
use super::super::mp4::{self, AudioFragment, Mp4Writer};
use super::timeline::{DecodeOrigin, Timeline};

/// One root-owned worker shared by background acquisition, seeks and complete-file export.
/// Dropping a waiting request does not drop the worker or release its preparation slot.
type SegmentPreparation =
    Shared<BoxFuture<'static, std::result::Result<Arc<PreparedSegment>, Arc<Error>>>>;

struct Segment {
    source: MediaResource,

    header: OnceCell<std::result::Result<Arc<AudioFragment>, Arc<Error>>>,

    prepared: OnceLock<SegmentPreparation>,
}

/// A fully decrypted segment MP4 shared by packet readers and complete-file export.
pub(super) struct PreparedSegment {
    pub bytes: Arc<[u8]>,

    /// Exact interval from playback tick zero, using the initialization timescale.
    pub timeline: Range<u64>,

    pub fragment: Arc<AudioFragment>,

    /// Start of clear AAC bytes in the in-memory MP4, before its sample-table metadata.
    pub payload_offset: u64,
}

struct Progress {
    timeline: Timeline,

    /// Only completely decrypted and remuxed segments are playable.
    prepared: Vec<bool>,
}

/// Owns the in-memory segments, content key and preparation tasks for one open track.
/// Only packet and complete-file readers retain DropGuards; capture does not keep downloads alive.
/// Dropping the last reader cancels unfinished work, which releases its shared buffers on exit.
pub(crate) struct SegmentStore {
    pub cancellation: CancellationToken,

    pub runtime: Handle,

    pub transfer: TransferState,

    segments: Vec<Segment>,

    progress: Mutex<Progress>,

    /// Materialized only for encoded-file reads; capture writes directly from the segments.
    pub(super) encoded_file: OnceCell<std::result::Result<Arc<[u8]>, Arc<Error>>>,

    pub(super) initialization: AudioInitialization,

    key: Zeroizing<[u8; 16]>,

    client: Client,

    header_prefetch: NonZeroUsize,

    concurrency: NonZeroUsize,

    preparation_slots: Semaphore,
}

impl SegmentStore {
    pub(crate) fn new(
        segments: Vec<MediaSegment>,
        initialization: AudioInitialization,
        key: Zeroizing<[u8; 16]>,
        client: Client,
        cancellation: CancellationToken,
        header_prefetch: NonZeroUsize,
        concurrency: NonZeroUsize,
    ) -> Result<Arc<Self>> {
        let timeline = Timeline::new(
            segments.iter().map(|segment| segment.duration),
            initialization.timescale().get(),
        )?;
        let count = segments.len();
        let store = Arc::new(Self {
            progress: Mutex::new(Progress {
                timeline,
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
            encoded_file: OnceCell::new(),
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

    pub(super) fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// Wait for every segment before a complete-file consumer starts writing.
    pub(super) async fn complete_segments(
        self: &Arc<Self>,
        request: CancellationToken,
    ) -> Result<Vec<Arc<PreparedSegment>>> {
        let mut prepared = Vec::with_capacity(self.segment_count());
        for index in 0..self.segment_count() {
            prepared.push(self.segment(index, request.clone()).await?);
        }
        Ok(prepared)
    }

    pub(super) fn exact_duration_ticks(&self) -> Option<u64> {
        self.progress.lock().timeline.exact_duration_ticks()
    }

    pub(super) fn timescale(&self) -> u32 {
        self.progress.lock().timeline.timescale
    }

    pub(super) fn declared_duration(&self) -> Duration {
        self.progress.lock().timeline.declared_duration
    }

    /// The producer owns acquisition independently of decoder reads and optional capture.
    pub(crate) fn acquire(self: &Arc<Self>) {
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

    pub(crate) async fn startup(self: &Arc<Self>, target: Duration) -> Result<()> {
        let estimated_count = self.progress.lock().timeline.startup_count(target);
        let initial = stream::iter(0..estimated_count)
            .map(|index| self.segment(index, self.cancellation.clone()))
            .buffered(self.concurrency.get());
        let remaining = stream::iter(estimated_count..self.segments.len())
            .then(|index| self.segment(index, self.cancellation.clone()));
        let mut startup = initial.chain(remaining).boxed();
        // EXTINF plans the initial window; actual sample durations decide readiness.
        while let Some(segment) = startup.next().await {
            let segment = segment?;
            if self.progress.lock().timeline.duration(segment.timeline.end) >= target {
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
        let mut index = self.progress.lock().timeline.estimated_segment(ticks);
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

    async fn header(&self, index: usize) -> Result<Arc<AudioFragment>> {
        let segment = self.segments.get(index).ok_or(Error::InvalidMedia {
            field: "segment index",
        })?;
        tokio::select! {
            biased;
            () = self.cancellation.cancelled() => Err(Error::Cancelled),
            result = segment.header.get_or_init(|| async {
                let result = async {
                    let header = cdn::fragment_header(&self.client, &segment.source, self.header_prefetch).await?;
                    AudioFragment::parse(&header, &self.initialization)
                }.await;
                result.map(Arc::new).map_err(Arc::new)
            }) => result.clone().map_err(Error::Preparation),
        }
    }

    /// Anchor explicit tfdt values to the first fragment, even when headers arrive out of order.
    /// Missing tfdt inherits the preceding playback interval, already relative to that origin.
    async fn timeline(&self, index: usize, fragment: &AudioFragment) -> Result<Range<u64>> {
        let first = self.header(0).await?;
        let origin = DecodeOrigin::new(first.decode_start);
        let mut unresolved = Vec::new();
        let mut current = index;
        let mut start = origin.playback_start(fragment.decode_start)?;
        while start.is_none() {
            if current == 0 {
                start = Some(0);
                break;
            }
            current -= 1;
            let known = self.progress.lock().timeline.known(current)?;
            if let Some(known) = known {
                start = Some(known.end);
                break;
            }
            let preceding = self.header(current).await?;
            start = origin.playback_start(preceding.decode_start)?;
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
            progress.timeline.record(position, start..end)?;
            start = end;
        }
        let end = start
            .checked_add(fragment.duration)
            .ok_or(Error::Overflow)?;
        progress.timeline.record(index, start..end)?;
        Ok(start..end)
    }

    async fn prepare_segment(self: &Arc<Self>, index: usize) -> Result<PreparedSegment> {
        let segment = self.segments.get(index).ok_or(Error::InvalidMedia {
            field: "segment index",
        })?;
        let bytes = download(&self.client, &segment.source).await?;
        let fragment = segment
            .header
            .get_or_init(|| async {
                let result = mp4::fragment_from_bytes(&bytes)
                    .and_then(|header| AudioFragment::parse(&header, &self.initialization));
                result.map(Arc::new).map_err(Arc::new)
            })
            .await
            .clone()
            .map_err(Error::Preparation)?;
        let timeline = self.timeline(index, &fragment).await?;
        let resource_offset = segment.source.resource_offset()?;
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            store.prepare_segment_buffer(index, &bytes, resource_offset, fragment, timeline)
        })
        .await?
    }

    /// Decrypts and remuxes in memory, off the async I/O workers.
    fn prepare_segment_buffer(
        &self,
        index: usize,
        bytes: &[u8],
        resource_offset: u64,
        fragment: Arc<AudioFragment>,
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
        let mut output = Cursor::new(Vec::new());
        let mut muxer = Mp4Writer::new(&mut output, &self.initialization)?;
        let payload_offset = muxer.position();
        muxer.append(&mut output, &fragment, &mut clear.as_slice())?;
        muxer.finish(&mut output)?;
        let bytes = Arc::from(output.into_inner());
        if self.cancellation.is_cancelled() {
            return Err(Error::Cancelled);
        }
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
            bytes,
            timeline,
            fragment,
            payload_offset,
        })
    }

    fn publish_progress(&self, progress: &Progress) {
        let count = progress.prepared.iter().take_while(|ready| **ready).count();
        let end = progress.timeline.prepared_prefix_end(count);
        // EXTINF declares the duration until all exact sample timings are known.
        let total = if count == self.segments.len() {
            end
        } else {
            progress.timeline.declared_duration.max(end)
        };
        self.transfer
            .set_buffered(BufferedPrefix::Time { end, total });
        if count == self.segments.len() {
            self.transfer.mark_complete();
        }
    }
}
