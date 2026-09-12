//! Encoded-media decoder construction and playback instance cancellation.

use std::io::{Read, Seek};
use std::time::Duration;

use color_eyre::eyre::eyre;
use rodio::Source;
use rodio::decoder::DecoderBuilder;
use rodio::source::SeekError;
use tokio_util::sync::CancellationToken;

/// Source wrapper ending immediately after playback instance cancellation.
pub(super) struct InstanceSource<S> {
    /// Decoder producing PCM samples.
    inner: S,

    /// Playback instance cancellation token.
    cancellation: CancellationToken,
}

impl<S> InstanceSource<S> {
    /// Wraps one decoder with its instance token.
    pub(super) fn new(inner: S, cancellation: CancellationToken) -> Self {
        Self {
            inner,
            cancellation,
        }
    }
}

impl<S> Iterator for InstanceSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cancellation.is_cancelled() {
            None
        } else {
            self.inner.next()
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S> Source for InstanceSource<S>
where
    S: Source<Item = f32>,
{
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        if self.cancellation.is_cancelled() {
            return Err(SeekError::NotSupported {
                underlying_source: std::any::type_name::<Self>(),
            });
        }
        self.inner.try_seek(position)
    }
}

/// Builds a rodio decoder and enables arbitrary seek only when byte length is known.
pub(crate) fn build_decoder<R>(
    reader: R,
    byte_len: Option<u64>,
) -> color_eyre::Result<rodio::Decoder<R>>
where
    R: Read + Seek + Send + Sync + 'static,
{
    let mut builder = DecoderBuilder::new().with_data(reader);
    if let Some(length) = byte_len {
        builder = builder.with_byte_len(length);
    }
    builder.build().map_err(|error| eyre!("decode: {error}"))
}
