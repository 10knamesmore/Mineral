//! Bounded PCM shared by decode workers and the device callback.
//!
//! Workers perform IO and DSP outside the lock. The callback only consumes prepared frames;
//! completed slots remain owned here until the engine removes them off the callback thread.

use std::collections::VecDeque;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use cpal::{FromSample, Sample};
use mineral_common::revision::Revision;
use mineral_playback::TransferState;
use spin::Mutex;
use tokio_util::sync::CancellationToken;

use crate::decoder::PcmFormat;
use crate::tap::TapWriter;
use crate::{Error, Result};

pub(crate) struct SharedPlayback {
    pub(super) state: Mutex<PlaybackState>,

    buffer_ms: NonZeroU32,
}

pub(super) struct OutputFormat {
    pub(super) pcm: PcmFormat,

    pub(super) generation: Revision,

    pub(super) capacity_samples: usize,
}

pub(super) struct PlaybackState {
    pub(super) tracks: [Option<Track>; 2],

    pub(super) current: Option<TrackSlot>,

    pub(super) next: Option<TrackSlot>,

    pub(super) output: Option<OutputFormat>,

    pub(super) paused: bool,

    /// Whether the last device callback received media frames rather than only silence.
    pub(super) rendering: bool,

    pub(super) gain: f32,

    pub(super) track_token: Revision,

    pub(super) finished_seq: u64,

    pub(super) last_position: Duration,

    tap: TapWriter,
}

/// The two reusable buffers alternate between current and prefetched playback.
#[derive(Clone, Copy)]
pub(super) enum TrackSlot {
    First,

    Second,
}

impl TrackSlot {
    pub(super) fn other(self) -> Self {
        match self {
            Self::First => Self::Second,
            Self::Second => Self::First,
        }
    }
}

pub(super) struct Track {
    pub(super) id: u64,

    /// Changed by seek or a new device format; workers discard obsolete work.
    pub(super) generation: Revision,

    pub(super) cancellation: CancellationToken,

    pub(super) transfer: Option<TransferState>,

    pub(super) source_format: Option<PcmFormat>,

    pub(super) duration: Option<Duration>,

    pub(super) samples: VecDeque<f32>,

    pub(super) origin: Duration,

    pub(super) rendered_frames: u64,

    pub(super) seek_position: Option<Duration>,

    pub(super) decoding: Decoding,

    /// Set by the callback only after the final prepared frame was consumed.
    pub(super) finished: bool,
}

pub(super) enum Decoding {
    Reading,

    Ended,

    Failed(Arc<Error>),
}

impl SharedPlayback {
    pub(crate) fn new(buffer_ms: NonZeroU32, gain: f32, tap: TapWriter) -> Self {
        Self {
            state: Mutex::new(PlaybackState {
                tracks: [None, None],
                current: None,
                next: None,
                output: None,
                paused: true,
                rendering: false,
                gain,
                track_token: Revision::default(),
                finished_seq: 0,
                last_position: Duration::ZERO,
                tap,
            }),
            buffer_ms,
        }
    }

    /// Commit a successfully opened device. PCM for another format is regenerated from
    /// the rendered position, rather than from the decoder's read-ahead position.
    pub(super) fn configure(&self, pcm: PcmFormat, generation: Revision) -> Result<()> {
        let frames =
            (u64::from(pcm.sample_rate.get()) * u64::from(self.buffer_ms.get())).div_ceil(1_000);
        let capacity_samples = usize::try_from(frames * u64::from(pcm.channels.get()))
            .map_err(|source| Error::BufferCapacity { source })?;
        let mut state = self.state.lock();
        if let Some(previous) = &state.output
            && previous.pcm != pcm
        {
            let rate = previous.pcm.sample_rate;
            for track in state.tracks.iter_mut().flatten() {
                if !track.finished && !matches!(track.decoding, Decoding::Failed(_)) {
                    let position = track.seek_position.unwrap_or_else(|| track.position(rate));
                    track.rewind(position, rate);
                    track.samples = VecDeque::with_capacity(capacity_samples);
                }
            }
            state.rendering = false;
        }
        state.tap.set_sample_rate(pcm.sample_rate);
        state.output = Some(OutputFormat {
            pcm,
            generation,
            capacity_samples,
        });
        drop(state);
        Ok(())
    }

    /// Contention or missing PCM produces silence without advancing the media position.
    /// No IO, decoding, allocation, or source destruction occurs in this callback.
    pub(super) fn render<T>(&self, data: &mut [T], pcm: PcmFormat, generation: Revision)
    where
        T: Sample + FromSample<f32>,
    {
        data.fill(T::EQUILIBRIUM);
        let Some(mut state) = self.state.try_lock() else {
            return;
        };
        if state.paused
            || !state
                .output
                .as_ref()
                .is_some_and(|output| output.generation == generation && output.pcm == pcm)
        {
            return;
        }
        state.render(data, pcm);
        drop(state);
    }
}

impl Track {
    pub(super) fn position(&self, rate: NonZeroU32) -> Duration {
        self.origin + frames_duration(self.rendered_frames, rate)
    }

    pub(super) fn rewind(&mut self, position: Duration, rate: NonZeroU32) {
        let rendered_position = self.position(rate);
        self.generation = self.generation.next();
        self.samples.clear();
        self.origin = rendered_position;
        self.rendered_frames = 0;
        self.seek_position = Some(position);
        self.decoding = Decoding::Reading;
        self.finished = false;
    }
}

impl Drop for Track {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

impl PlaybackState {
    pub(super) fn track(&self, slot: TrackSlot) -> &Option<Track> {
        match slot {
            TrackSlot::First => &self.tracks[0],
            TrackSlot::Second => &self.tracks[1],
        }
    }

    pub(super) fn track_mut(&mut self, slot: TrackSlot) -> &mut Option<Track> {
        match slot {
            TrackSlot::First => &mut self.tracks[0],
            TrackSlot::Second => &mut self.tracks[1],
        }
    }

    pub(super) fn stop(&mut self) {
        self.current = None;
        self.next = None;
        self.tracks = [None, None];
        self.paused = true;
        self.rendering = false;
        self.last_position = Duration::ZERO;
    }

    fn render<T>(&mut self, data: &mut [T], pcm: PcmFormat)
    where
        T: Sample + FromSample<f32>,
    {
        let channels = usize::from(pcm.channels.get());
        let gain = self.gain;
        self.rendering = false;
        let mut frames = data.chunks_exact_mut(channels).peekable();
        while frames.peek().is_some() {
            let Some(index) = self.current else {
                break;
            };
            let Some(track) = self.track_mut(index).as_mut() else {
                break;
            };
            if matches!(track.decoding, Decoding::Failed(_)) {
                self.paused = true;
                break;
            }
            if track.samples.is_empty() {
                if !matches!(track.decoding, Decoding::Ended) {
                    break;
                }
                track.finished = true;
                self.last_position = track.position(pcm.sample_rate);
                self.finished_seq += 1;
                self.current = self.next.take();
                if self.current.is_some() {
                    self.track_token = self.track_token.next();
                }
                continue;
            }
            let Some(frame) = frames.next() else {
                break;
            };
            let mut mono = 0.0;
            for (output, sample) in frame.iter_mut().zip(track.samples.drain(..channels)) {
                mono += sample;
                *output = T::from_sample(sample * gain);
            }
            track.rendered_frames += 1;
            self.rendering = true;
            self.tap.push(mono / f32::from(pcm.channels.get()));
        }
    }
}

pub(super) fn frames_duration(frames: u64, rate: NonZeroU32) -> Duration {
    let rate = u64::from(rate.get());
    Duration::from_secs(frames / rate)
        + Duration::from_nanos((frames % rate) * 1_000_000_000 / rate)
}
