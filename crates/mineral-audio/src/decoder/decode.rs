use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

use mineral_playback::{AudioInput, AudioPacketSource};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::formats::{SeekMode, SeekTo};
use symphonia::core::units::{Duration as TrackDuration, Time, TimeBase, Timestamp};
use symphonia::default::get_codecs;

use super::error::DecoderError;
use super::packets;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct PcmFormat {
    pub(crate) channels: NonZeroU16,

    pub(crate) sample_rate: NonZeroU32,
}

pub(crate) struct DecodedAudio {
    /// Interleaved samples containing only complete frames.
    pub(crate) samples: Vec<f32>,

    pub(crate) format: PcmFormat,

    /// Time of the first frame, relative to the track's playback start.
    pub(crate) position: Duration,
}

/// Decodes one audio track, retaining the first block until the first read.
pub(crate) struct Decoder {
    packets: AudioPackets,

    pending: Option<DecodedAudio>,

    format: PcmFormat,

    time_base: Option<TimeBase>,

    start_ts: Timestamp,

    next_position: Duration,

    /// Fractional nanosecond numerator, with the current sample rate as denominator.
    position_remainder: u32,
}

impl Decoder {
    pub(crate) fn from_input(input: AudioInput) -> Result<Self, DecoderError> {
        let source = packets::open(input)?;
        let track = source.track();
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
        let mut packets = AudioPackets {
            source,
            codec,
            track_id,
        };
        let buffer = packets.next_audio()?.ok_or(DecoderError::NoAudioFrames)?;
        let format = buffer.format;
        let pending = Some(DecodedAudio {
            samples: buffer.samples,
            format,
            position: Duration::ZERO,
        });

        Ok(Self {
            packets,
            pending,
            format,
            time_base,
            start_ts,
            next_position: Duration::ZERO,
            position_remainder: 0,
        })
    }

    pub(crate) fn format(&self) -> PcmFormat {
        self.format
    }

    pub(crate) fn duration(&self) -> Option<Duration> {
        self.time_base
            .zip(self.track_duration())
            .and_then(|(time_base, duration)| time_base.calc_duration(duration))
            .and_then(|time| {
                let (seconds, nanos) = time.parts();
                u64::try_from(seconds)
                    .ok()
                    .map(|seconds| Duration::new(seconds, nanos))
            })
    }

    fn track_duration(&self) -> Option<TrackDuration> {
        // Fragmented headers can leave duration at zero while audio follows.
        self.packets
            .source
            .track()
            .duration
            .filter(|duration| !duration.is_zero())
    }

    fn end_timestamp(&self, position: Duration) -> Result<Option<Timestamp>, DecoderError> {
        self.track_duration()
            .filter(|_| self.duration() == Some(position))
            .map(|duration| {
                self.start_ts
                    .checked_add(duration)
                    .ok_or(DecoderError::TimingOverflow)
            })
            .transpose()
    }

    pub(crate) fn read(&mut self) -> Result<Option<DecodedAudio>, DecoderError> {
        let audio = match self.pending.take() {
            Some(audio) => audio,
            None => {
                let Some(buffer) = self.packets.next_audio()? else {
                    return Ok(None);
                };
                DecodedAudio {
                    samples: buffer.samples,
                    format: buffer.format,
                    position: self.next_position,
                }
            }
        };
        if audio.format.sample_rate != self.format.sample_rate {
            self.position_remainder = 0;
        }
        self.format = audio.format;
        let frames = audio.samples.len() / usize::from(audio.format.channels.get());
        let frame_nanos = u128::try_from(frames)
            .map_err(DecoderError::TimingConversion)?
            .checked_mul(1_000_000_000)
            .and_then(|nanos| nanos.checked_add(u128::from(self.position_remainder)))
            .ok_or(DecoderError::TimingOverflow)?;
        let rate = u128::from(audio.format.sample_rate.get());
        let nanos = frame_nanos / rate;
        self.position_remainder =
            u32::try_from(frame_nanos % rate).map_err(DecoderError::TimingConversion)?;
        let elapsed = Duration::new(
            u64::try_from(nanos / 1_000_000_000).map_err(DecoderError::TimingConversion)?,
            u32::try_from(nanos % 1_000_000_000).map_err(DecoderError::TimingConversion)?,
        );
        self.next_position = audio
            .position
            .checked_add(elapsed)
            .ok_or(DecoderError::TimingOverflow)?;
        Ok(Some(audio))
    }

    /// Seeks to a frame boundary at or after the requested time. The returned
    /// position is clamped to the known duration and labels the next block.
    /// A missing time base leaves decoding untouched; an attempted demuxer seek
    /// cannot be rolled back on failure.
    pub(crate) fn seek(&mut self, position: Duration) -> Result<Duration, DecoderError> {
        let time_base = self.time_base.ok_or(DecoderError::SeekUnsupported)?;
        let position = self
            .duration()
            .map_or(position, |duration| position.min(duration));
        let end_ts = self.end_timestamp(position)?;
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
        self.pending = None;
        self.packets
            .source
            .seek(
                SeekMode::Accurate,
                SeekTo::Timestamp {
                    ts: seek_ts,
                    track_id: self.packets.track_id,
                },
            )
            .map_err(DecoderError::Seek)?;
        self.packets.codec.reset();

        // Seeking can discover an exact final segment after a declared duration
        // was used initially. Endpoint trimming must use the newly known sample end.
        let position = self.duration().map_or(position, |duration| {
            if end_ts.is_some() {
                duration
            } else {
                position.min(duration)
            }
        });
        let target = SeekTarget {
            position,
            start_ts: self.start_ts,
            end_ts: self.end_timestamp(position)?,
            time_base,
        };
        let mut reached_target = false;
        loop {
            let Some(mut buffer) = self.packets.next_audio()? else {
                if !reached_target {
                    return Err(DecoderError::SeekPastEnd { position });
                }
                self.next_position = position;
                self.position_remainder = 0;
                return Ok(position);
            };
            let frames_to_skip = target.frames_to_skip(&buffer)?;
            let channels = usize::from(buffer.format.channels.get());
            let frames = u128::try_from(buffer.samples.len() / channels)
                .map_err(DecoderError::TimingConversion)?;
            if frames_to_skip >= frames {
                reached_target = frames_to_skip == frames;
                continue;
            }
            let samples_to_skip =
                usize::try_from(frames_to_skip).map_err(DecoderError::TimingConversion)? * channels;
            buffer.samples.drain(..samples_to_skip);
            self.format = buffer.format;
            self.pending = Some(DecodedAudio {
                samples: buffer.samples,
                format: buffer.format,
                position,
            });
            self.next_position = position;
            self.position_remainder = 0;
            return Ok(position);
        }
    }
}

struct AudioPackets {
    source: Box<dyn AudioPacketSource>,

    codec: Box<dyn AudioDecoder>,

    track_id: u32,
}

impl AudioPackets {
    fn next_audio(&mut self) -> Result<Option<AudioBuffer>, DecoderError> {
        loop {
            let Some(packet) = self
                .source
                .next_packet()
                .map_err(DecoderError::ReadPacket)?
            else {
                return Ok(None);
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let decoded = self
                .codec
                .decode(&packet)
                .map_err(DecoderError::DecodePacket)?;
            if decoded.frames() == 0 {
                continue;
            }
            let channels = decoded.spec().channels().count();
            let channels = u16::try_from(channels)
                .ok()
                .and_then(NonZeroU16::new)
                .ok_or(DecoderError::InvalidChannels { channels })?;
            let sample_rate =
                NonZeroU32::new(decoded.spec().rate()).ok_or(DecoderError::InvalidSampleRate)?;
            let start_ts = packet
                .pts
                .checked_add(packet.trim_start)
                .ok_or(DecoderError::TimingOverflow)?;
            let mut samples = vec![0.0; decoded.samples_interleaved()];
            decoded.copy_to_slice_interleaved(&mut samples);
            return Ok(Some(AudioBuffer {
                samples,
                format: PcmFormat {
                    channels,
                    sample_rate,
                },
                start_ts,
            }));
        }
    }
}

struct AudioBuffer {
    samples: Vec<f32>,

    format: PcmFormat,

    /// The first retained frame, after the codec's gapless start trim.
    start_ts: Timestamp,
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
            .checked_mul(u128::from(buffer.format.sample_rate.get()))
            .ok_or(DecoderError::TimingOverflow)?;
        let denominator = u128::from(self.time_base.denom.get()) * 1_000_000_000;
        Ok(numerator.div_ceil(denominator))
    }
}
