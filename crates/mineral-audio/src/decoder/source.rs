//! Decode encoded media into Rodio spans without discarding terminal failures.

use std::any::type_name;
use std::io::{Read, Seek};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use rodio::source::SeekError;
use rodio::{ChannelCount, SampleRate, Source};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Duration as TrackDuration, Time, TimeBase, Timestamp};
use symphonia::default::{get_codecs, get_probe};

use super::error::DecoderError;
use super::reader::EncodedReader;

/// The reason an audio source stopped producing samples.
#[derive(Clone)]
pub(crate) enum DecodeOutcome {
    /// The demuxer reached its declared end without a read or decode failure.
    Completed,

    /// The source stopped after an unrecoverable read, decode, or seek error.
    Failed(Arc<DecoderError>),
}

/// Shared with the engine after the decoder moves into Rodio's playback thread.
#[derive(Clone)]
pub(crate) struct DecoderStatus {
    outcome: Arc<Mutex<Option<DecodeOutcome>>>,
}

impl DecoderStatus {
    pub(crate) fn outcome(&self) -> Option<DecodeOutcome> {
        self.outcome.lock().clone()
    }
}

/// Owns one audio track and exposes decoded spans to rodio.
pub(crate) struct Decoder {
    packets: AudioPackets,

    buffer: AudioBuffer,

    time_base: Option<TimeBase>,

    start_ts: Timestamp,

    track_duration: Option<TrackDuration>,

    total_duration: Option<Duration>,

    status: DecoderStatus,

    /// Local copy avoids locking the engine's shared result on every sample.
    terminal: Option<DecodeOutcome>,
}

impl Decoder {
    pub(crate) fn new<R>(reader: R) -> Result<Self, DecoderError>
    where
        R: Read + Seek + Send + Sync + 'static,
    {
        let reader = EncodedReader::new(reader)?;
        let stream = MediaSourceStream::new(Box::new(reader), Default::default());
        let format = get_probe()
            .probe(
                &Hint::new(),
                stream,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(DecoderError::Probe)?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or(DecoderError::NoAudioTrack)?;
        let parameters = track
            .codec_params
            .as_ref()
            .and_then(|parameters| parameters.audio())
            .ok_or(DecoderError::NoAudioTrack)?;
        let codec = get_codecs()
            .make_audio_decoder(parameters, &AudioDecoderOptions::default())
            .map_err(DecoderError::CreateCodec)?;
        let track_id = track.id;
        let time_base = track.time_base;
        let start_ts = track.start_ts;
        // Fragmented MP4 headers may leave duration at zero while audio follows.
        let track_duration = track.duration.filter(|duration| !duration.is_zero());
        let total_duration = time_base
            .zip(track_duration)
            .and_then(|(time_base, duration)| time_base.calc_duration(duration))
            .and_then(|time| {
                let (seconds, nanos) = time.parts();
                u64::try_from(seconds)
                    .ok()
                    .map(|seconds| Duration::new(seconds, nanos))
            });
        let mut packets = AudioPackets {
            format,
            codec,
            track_id,
        };
        let buffer = packets.next_audio()?.ok_or(DecoderError::NoAudioFrames)?;

        Ok(Self {
            packets,
            buffer,
            time_base,
            start_ts,
            track_duration,
            total_duration,
            status: DecoderStatus {
                outcome: Arc::new(Mutex::new(None)),
            },
            terminal: None,
        })
    }

    pub(crate) fn status(&self) -> DecoderStatus {
        self.status.clone()
    }

    fn finish(&mut self, outcome: DecodeOutcome) {
        self.buffer.samples.clear();
        self.buffer.next_sample = 0;
        self.terminal = Some(outcome.clone());
        *self.status.outcome.lock() = Some(outcome);
    }

    fn fail(&mut self, error: DecoderError) -> Arc<DecoderError> {
        let error = Arc::new(error);
        self.finish(DecodeOutcome::Failed(error.clone()));
        error
    }

    fn refill(&mut self) {
        match self.packets.next_audio() {
            Ok(Some(buffer)) => self.buffer = buffer,
            Ok(None) => self.finish(DecodeOutcome::Completed),
            Err(error) => {
                self.fail(error);
            }
        }
    }
}

impl Iterator for Decoder {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.terminal.is_some() {
            return None;
        }

        let sample = self.buffer.samples.get(self.buffer.next_sample).copied()?;
        self.buffer.next_sample += 1;
        if self.buffer.remaining_samples() == 0 {
            // Publish the next span's format, or its terminal outcome, before Rodio
            // checks exhaustion. No lock is taken while consuming buffered samples.
            self.refill();
        }
        Some(sample)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.terminal.is_some() {
            (0, Some(0))
        } else {
            (self.buffer.remaining_samples(), None)
        }
    }
}

impl Source for Decoder {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.buffer.remaining_samples())
    }

    fn channels(&self) -> ChannelCount {
        self.buffer.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.buffer.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.total_duration
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        self.seek(position)
    }
}

struct AudioPackets {
    format: Box<dyn FormatReader>,

    codec: Box<dyn AudioDecoder>,

    track_id: u32,
}

impl AudioPackets {
    fn next_audio(&mut self) -> Result<Option<AudioBuffer>, DecoderError> {
        loop {
            let Some(packet) = self
                .format
                .next_packet()
                .map_err(DecoderError::ReadPacket)?
            else {
                return Ok(None);
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let decoded = match self.codec.decode(&packet) {
                Ok(decoded) => decoded,
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(error) => return Err(DecoderError::DecodePacket(error)),
            };
            if decoded.frames() == 0 {
                continue;
            }
            let channels = decoded.spec().channels().count();
            let channels = u16::try_from(channels)
                .ok()
                .and_then(ChannelCount::new)
                .ok_or(DecoderError::InvalidChannels { channels })?;
            let sample_rate =
                SampleRate::new(decoded.spec().rate()).ok_or(DecoderError::InvalidSampleRate)?;
            let start_ts = packet
                .pts
                .checked_add(packet.trim_start)
                .ok_or(DecoderError::TimingOverflow)?;
            let mut samples = vec![0.0; decoded.samples_interleaved()];
            decoded.copy_to_slice_interleaved(&mut samples);
            return Ok(Some(AudioBuffer {
                samples,
                next_sample: 0,
                channels,
                sample_rate,
                start_ts,
            }));
        }
    }
}

struct AudioBuffer {
    samples: Vec<f32>,

    next_sample: usize,

    channels: ChannelCount,

    sample_rate: SampleRate,

    /// The first retained frame, after the codec's gapless start trim.
    start_ts: Timestamp,
}

impl AudioBuffer {
    fn remaining_samples(&self) -> usize {
        self.samples.len() - self.next_sample
    }
}

impl Decoder {
    /// Unsupported seeks leave playback intact. Once a seek is attempted, any
    /// failure terminates this source; demuxer state cannot be rolled back.
    fn seek(&mut self, position: Duration) -> Result<(), SeekError> {
        if let Some(DecodeOutcome::Failed(error)) = &self.terminal {
            return Err(SeekError::Other(error.clone()));
        }
        let Some(time_base) = self.time_base else {
            return Err(SeekError::NotSupported {
                underlying_source: type_name::<Self>(),
            });
        };
        let active_channel = self.buffer.next_sample % usize::from(self.buffer.channels.get());

        // A successful seek starts another decoding run, including after EOF.
        if self.terminal.take().is_some() {
            *self.status.outcome.lock() = None;
        }
        self.seek_audio(position, time_base, active_channel)
            .map_err(|error| SeekError::Other(self.fail(error)))
    }

    fn seek_audio(
        &mut self,
        position: Duration,
        time_base: TimeBase,
        active_channel: usize,
    ) -> Result<(), DecoderError> {
        let position = self
            .total_duration
            .map_or(position, |duration| position.min(duration));
        let end_ts = self
            .track_duration
            .filter(|_| self.total_duration == Some(position))
            .map(|duration| {
                self.start_ts
                    .checked_add(duration)
                    .ok_or(DecoderError::TimingOverflow)
            })
            .transpose()?;
        let target_ts = match end_ts {
            Some(timestamp) => timestamp,
            None => {
                let time = Time::try_from_nanos_u128(position.as_nanos())
                    .ok_or(DecoderError::TimingOverflow)?;
                let relative_ts = time_base
                    .calc_timestamp(time)
                    .ok_or(DecoderError::TimingOverflow)?;
                Timestamp::new(
                    self.start_ts
                        .get()
                        .checked_add(relative_ts.get())
                        .ok_or(DecoderError::TimingOverflow)?,
                )
            }
        };
        // Seek one tick earlier so an endpoint seek can decode the last frame
        // and verify EOF, even when the duration is unknown.
        let seek_ts = if target_ts > self.start_ts {
            target_ts
                .checked_sub(TrackDuration::new(1))
                .ok_or(DecoderError::TimingOverflow)?
        } else {
            target_ts
        };
        self.packets
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Timestamp {
                    ts: seek_ts,
                    track_id: self.packets.track_id,
                },
            )
            .map_err(DecoderError::Seek)?;
        self.packets.codec.reset();

        let target = SeekTarget {
            position,
            start_ts: self.start_ts,
            end_ts,
            time_base,
        };
        let mut reached_target = false;
        loop {
            let Some(mut buffer) = self.packets.next_audio()? else {
                if !reached_target {
                    return Err(DecoderError::SeekPastEnd { position });
                }
                self.finish(DecodeOutcome::Completed);
                return Ok(());
            };
            let frames_to_skip = target.frames_to_skip(&buffer)?;
            let frames = u128::try_from(buffer.samples.len() / usize::from(buffer.channels.get()))
                .map_err(DecoderError::TimingConversion)?;
            if frames_to_skip >= frames {
                reached_target = frames_to_skip == frames;
                continue;
            }
            let channels = usize::from(buffer.channels.get());
            // Rodio may request a seek between channels of one frame.
            buffer.next_sample =
                usize::try_from(frames_to_skip).map_err(DecoderError::TimingConversion)? * channels
                    + active_channel % channels;
            self.buffer = buffer;
            return Ok(());
        }
    }
}

struct SeekTarget {
    position: Duration,

    start_ts: Timestamp,

    end_ts: Option<Timestamp>,

    time_base: TimeBase,
}

impl SeekTarget {
    fn frames_to_skip(&self, buffer: &AudioBuffer) -> Result<u128, DecoderError> {
        let tick_nanos = i128::from(self.time_base.numer.get()) * 1_000_000_000;
        let denominator = i128::from(self.time_base.denom.get());
        let origin = self.end_ts.unwrap_or(self.start_ts);
        let tick_delta = i128::from(origin.get()) - i128::from(buffer.start_ts.get());
        let elapsed = if self.end_ts.is_some() {
            0
        } else {
            i128::try_from(self.position.as_nanos())
                .map_err(DecoderError::TimingConversion)?
                .checked_mul(denominator)
                .ok_or(DecoderError::TimingOverflow)?
        };
        let difference = tick_delta
            .checked_mul(tick_nanos)
            .and_then(|difference| difference.checked_add(elapsed))
            .ok_or(DecoderError::TimingOverflow)?;
        if difference <= 0 {
            return Ok(0);
        }

        // Keep the packet timestamp rational until the final frame rounding.
        // Converting it to nanoseconds first can shift an exact seek by one frame.
        let numerator = u128::try_from(difference)
            .map_err(DecoderError::TimingConversion)?
            .checked_mul(u128::from(buffer.sample_rate.get()))
            .ok_or(DecoderError::TimingOverflow)?;
        let denominator = u128::from(self.time_base.denom.get()) * 1_000_000_000;
        Ok(numerator.div_ceil(denominator))
    }
}
