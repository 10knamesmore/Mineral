//! Worker-side channel mapping and continuous sample-rate conversion.

use std::collections::VecDeque;

use audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

use crate::decoder::PcmFormat;
use crate::{Error, Result};

/// One conversion run, reset on seek or format changes. Filter delay and padding
/// are removed so adjacent tracks retain their original duration.
pub(super) struct PcmConverter {
    pub(super) input_format: PcmFormat,

    output_format: PcmFormat,

    resampler: Option<Fft<f32>>,

    input: VecDeque<f32>,

    input_frames: u64,

    output_frames: u64,

    delay_remaining: usize,
}

impl PcmConverter {
    pub(super) fn new(
        input_format: PcmFormat,
        output_format: PcmFormat,
        chunk_frames: usize,
    ) -> Result<Self> {
        let resampler = if input_format.sample_rate == output_format.sample_rate {
            None
        } else {
            Some(
                Fft::new(
                    usize::try_from(input_format.sample_rate.get())
                        .map_err(|source| Error::BufferCapacity { source })?,
                    usize::try_from(output_format.sample_rate.get())
                        .map_err(|source| Error::BufferCapacity { source })?,
                    chunk_frames,
                    usize::from(output_format.channels.get()),
                    FixedSync::Input,
                )
                .map_err(|source| Error::Resampler { source })?,
            )
        };
        let delay_remaining = resampler.as_ref().map_or(0, Resampler::output_delay);
        Ok(Self {
            input_format,
            output_format,
            resampler,
            input: VecDeque::new(),
            input_frames: 0,
            output_frames: 0,
            delay_remaining,
        })
    }

    /// Mono is duplicated into the front pair; mono output averages the input.
    /// Otherwise existing channels keep their order and additional outputs are silent.
    pub(super) fn push(&mut self, samples: &[f32]) {
        let input_channels = usize::from(self.input_format.channels.get());
        let output_channels = usize::from(self.output_format.channels.get());
        for frame in samples.chunks_exact(input_channels) {
            if output_channels == 1 {
                self.input.push_back(
                    frame.iter().sum::<f32>() / f32::from(self.input_format.channels.get()),
                );
            } else {
                for channel in 0..output_channels {
                    let source_channel = if input_channels == 1 && channel < 2 {
                        0
                    } else {
                        channel
                    };
                    let sample = frame.get(source_channel).copied().unwrap_or(0.0);
                    self.input.push_back(sample);
                }
            }
            self.input_frames += 1;
        }
    }

    /// Returns the next converted chunk. At EOF, flushes the filter and trims
    /// padding to the frame count implied by the input/output sample-rate ratio.
    pub(super) fn next(&mut self, end: bool) -> Result<Option<Vec<f32>>> {
        let Some(resampler) = self.resampler.as_mut() else {
            return Ok((!self.input.is_empty()).then(|| self.input.drain(..).collect()));
        };
        let channels = usize::from(self.output_format.channels.get());
        let expected_frames = (self.input_frames * u64::from(self.output_format.sample_rate.get()))
            / u64::from(self.input_format.sample_rate.get());
        loop {
            if end && self.output_frames >= expected_frames {
                return Ok(None);
            }
            let needed = resampler.input_frames_next();
            let available = self.input.len() / channels;
            if !end && available < needed {
                return Ok(None);
            }
            let frames = available.min(needed);
            let input: Vec<f32> = self.input.drain(..frames * channels).collect();
            let input = InterleavedSlice::new(&input, channels, frames)
                .map_err(|source| Error::PcmBuffer { source })?;
            let output_capacity = resampler.output_frames_max();
            let mut output = vec![0.0; output_capacity * channels];
            let mut output_buffer =
                InterleavedSlice::new_mut(&mut output, channels, output_capacity)
                    .map_err(|source| Error::PcmBuffer { source })?;
            let indexing = Indexing::new().partial_len(frames);
            let (_, produced) = resampler
                .process_into_buffer(&input, &mut output_buffer, Some(&indexing))
                .map_err(|source| Error::Resample { source })?;
            let skip = self.delay_remaining.min(produced);
            self.delay_remaining -= skip;
            let remaining = produced - skip;
            let retained = if end {
                remaining.min(
                    usize::try_from(expected_frames - self.output_frames)
                        .map_err(|source| Error::BufferCapacity { source })?,
                )
            } else {
                remaining
            };
            output.drain(..skip * channels);
            output.truncate(retained * channels);
            self.output_frames +=
                u64::try_from(retained).map_err(|source| Error::BufferCapacity { source })?;
            if !output.is_empty() {
                return Ok(Some(output));
            }
        }
    }
}
