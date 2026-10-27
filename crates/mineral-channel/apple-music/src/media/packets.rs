use std::fs::{self, File};
use std::io;
use std::ops::Range;
use std::sync::Arc;

use mineral_playback::{AudioPacketSource, ReadControl};
use shiguredo_mp4::demux::{Input, Mp4FileDemuxer};
use symphonia::core::errors::{Error as DecodeError, Result as DecodeResult, SeekErrorKind};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, SeekMode, SeekTo, SeekedTo, Track, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::packet::Packet;
use symphonia::core::units::{Duration as TickDuration, TimeBase, Timestamp};
use symphonia::default::get_probe;
use tokio_util::sync::{CancellationToken, DropGuard};

use super::error::{Error, Result};
use super::store::{MediaStore, PreparedSegment};

/// 单个压缩 AAC 样本；时间戳属于整首歌的时间轴，而非分片局部时间轴。
struct AudioSample {
    /// 样本在已准备分片文件中的字节范围。
    bytes: Range<usize>,

    /// Decode Timestamp：解码时间戳，已包含分片在整首歌中的起始偏移。
    /// 单位为音轨 tick，由 track.time_base 换算为秒。
    dts: Timestamp,

    /// Presentation Timestamp：呈现时间戳，即播放时间。
    /// 与 DTS 单位相同；PTS = DTS + MP4 composition offset，未提供 offset 时等于 DTS。
    pts: Timestamp,

    /// 样本时长，单位同样为音轨 tick。
    duration: TickDuration,
}

struct SegmentPackets {
    index: usize,
    bytes: Vec<u8>,
    samples: Vec<AudioSample>,
    next: usize,
}

impl SegmentPackets {
    fn open(index: usize, prepared: &PreparedSegment) -> Result<Self> {
        let bytes = fs::read(&prepared.path)?;
        let mut demuxer = Mp4FileDemuxer::new();
        demuxer.handle_input(Input {
            position: 0,
            data: &bytes,
        });
        let mut samples = Vec::new();
        while let Some(sample) = demuxer.next_sample()? {
            let dts = prepared
                .timeline
                .start
                .checked_add(sample.timestamp)
                .ok_or(Error::Overflow)?;
            let dts = Timestamp::try_from(dts)?;
            let pts = match sample.composition_time_offset {
                Some(offset) => {
                    Timestamp::new(dts.get().checked_add(offset).ok_or(Error::Overflow)?)
                }
                None => dts,
            };
            let start = usize::try_from(sample.data_offset)?;
            let end = start.checked_add(sample.data_size).ok_or(Error::Overflow)?;
            samples.push(AudioSample {
                bytes: start..end,
                dts,
                pts,
                duration: TickDuration::new(u64::from(sample.duration)),
            });
        }
        Ok(Self {
            index,
            bytes,
            samples,
            next: 0,
        })
    }

    fn next_packet(&mut self, track_id: u32) -> Result<Option<Packet>> {
        let Some(sample) = self.samples.get(self.next) else {
            return Ok(None);
        };
        let data = self
            .bytes
            .get(sample.bytes.clone())
            .ok_or(Error::InvalidMedia {
                field: "AAC packet range",
            })?;
        let mut packet = Packet::new(track_id, sample.pts, sample.duration, data);
        packet.dts = sample.dts;
        self.next += 1;
        Ok(Some(packet))
    }
}

pub(super) struct AudioPackets {
    store: Arc<MediaStore>,
    control: ReadControl,
    track: Track,
    current: SegmentPackets,
    _lifetime: Arc<DropGuard>,
}

impl AudioPackets {
    /// Called on a blocking thread after the contiguous startup gate has opened.
    pub(super) fn new(
        store: Arc<MediaStore>,
        control: ReadControl,
        lifetime: Arc<DropGuard>,
    ) -> Result<Self> {
        let prepared = store
            .runtime
            .block_on(store.segment(0, control.request_cancellation()))?;
        let current = SegmentPackets::open(0, &prepared)?;
        let stream =
            MediaSourceStream::new(Box::new(File::open(&prepared.path)?), Default::default());
        let format = get_probe().probe(
            &Hint::new(),
            stream,
            FormatOptions::default(),
            Default::default(),
        )?;
        let mut track =
            format
                .default_track(TrackType::Audio)
                .cloned()
                .ok_or(Error::InvalidMedia {
                    field: "prepared AAC track",
                })?;
        let time_base = TimeBase::try_from_recip(store.timescale).ok_or(Error::InvalidMedia {
            field: "audio timescale",
        })?;
        if track.time_base != Some(time_base) || track.codec_params.is_none() {
            return Err(Error::InvalidMedia {
                field: "prepared AAC codec parameters",
            });
        }
        // The initialization has no complete sample table. EXTINF is only a declared
        // duration; packet timestamps and EOF come exclusively from actual samples.
        let ticks = match store.exact_duration_ticks() {
            Some(ticks) => ticks,
            None => {
                let ticks = store
                    .declared_duration
                    .as_nanos()
                    .checked_mul(u128::from(store.timescale))
                    .ok_or(Error::Overflow)?
                    / 1_000_000_000;
                u64::try_from(ticks)?
            }
        };
        track.duration = Some(TickDuration::new(ticks));
        track.num_frames = None;
        Ok(Self {
            store,
            control,
            track,
            current,
            _lifetime: lifetime,
        })
    }

    fn demand(&self) -> DecodeResult<CancellationToken> {
        let request = self.control.request_cancellation();
        self.check_demand(&request)?;
        Ok(request)
    }

    fn check_demand(&self, request: &CancellationToken) -> DecodeResult<()> {
        if self.store.cancellation.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::ConnectionAborted, Error::Cancelled).into());
        }
        if request.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, Error::Cancelled).into());
        }
        Ok(())
    }

    fn failure(&self, error: Error, request: &CancellationToken) -> DecodeError {
        match self.check_demand(request) {
            Err(error) => error,
            Ok(()) => io::Error::other(error).into(),
        }
    }

    fn update_duration(&mut self) {
        if let Some(ticks) = self.store.exact_duration_ticks() {
            self.track.duration = Some(TickDuration::new(ticks));
        }
    }

    fn open_segment(
        &self,
        index: usize,
        request: &CancellationToken,
    ) -> DecodeResult<SegmentPackets> {
        let prepared = self
            .store
            .runtime
            .block_on(self.store.segment(index, request.clone()))
            .map_err(|error| self.failure(error, request))?;
        let packets =
            SegmentPackets::open(index, &prepared).map_err(|error| self.failure(error, request))?;
        self.check_demand(request)?;
        Ok(packets)
    }
}

impl AudioPacketSource for AudioPackets {
    fn track(&self) -> &Track {
        &self.track
    }

    fn next_packet(&mut self) -> DecodeResult<Option<Packet>> {
        let request = self.demand()?;
        self.update_duration();
        loop {
            let packet = self
                .current
                .next_packet(self.track.id)
                .map_err(|error| self.failure(error, &request))?;
            if packet.is_some() {
                return Ok(packet);
            }
            let index = self.current.index + 1;
            if index == self.store.segment_count() {
                return Ok(None);
            }
            self.current = self.open_segment(index, &request)?;
        }
    }

    fn seek(&mut self, _mode: SeekMode, to: SeekTo) -> DecodeResult<SeekedTo> {
        let request = self.demand()?;
        let required_ts = match to {
            SeekTo::Time { time, track_id } => {
                if track_id.is_some_and(|id| id != self.track.id) {
                    return Err(DecodeError::SeekError(SeekErrorKind::InvalidTrack));
                }
                let time_base = self
                    .track
                    .time_base
                    .ok_or(DecodeError::SeekError(SeekErrorKind::Unseekable))?;
                time_base
                    .calc_timestamp(time)
                    .ok_or(DecodeError::SeekError(SeekErrorKind::OutOfRange))?
            }
            SeekTo::Timestamp { ts, track_id } => {
                if track_id != self.track.id {
                    return Err(DecodeError::SeekError(SeekErrorKind::InvalidTrack));
                }
                ts
            }
        };
        let ticks = u64::try_from(required_ts.get())
            .map_err(|_negative_timestamp| DecodeError::SeekError(SeekErrorKind::OutOfRange))?;
        let index = self
            .store
            .runtime
            .block_on(async {
                tokio::select! {
                    biased;
                    () = request.cancelled() => Err(Error::Cancelled),
                    result = self.store.locate(ticks) => result,
                }
            })
            .map_err(|error| self.failure(error, &request))?;
        let mut replacement = self.open_segment(index, &request)?;
        self.update_duration();
        loop {
            if let Some((position, sample)) = replacement
                .samples
                .iter()
                .enumerate()
                .rfind(|(_, sample)| sample.pts <= required_ts)
            {
                let actual_ts = sample.pts;
                replacement.next = position;
                self.check_demand(&request)?;
                self.current = replacement;
                return Ok(SeekedTo {
                    track_id: self.track.id,
                    required_ts,
                    actual_ts,
                });
            }
            if replacement.index == 0 {
                return Err(DecodeError::SeekError(SeekErrorKind::OutOfRange));
            }
            replacement = self.open_segment(replacement.index - 1, &request)?;
        }
    }
}
