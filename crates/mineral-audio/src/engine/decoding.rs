//! Encoded-media decoder construction and playback instance cancellation.

use std::io::{Read, Seek};
use std::sync::Arc;
use std::time::Duration;

use rodio::Source;
use rodio::source::SeekError;
use tokio_util::sync::CancellationToken;

use crate::decoder::{DecodeOutcome, Decoder, DecoderStatus};

/// Source wrapper ending immediately after playback instance cancellation.
pub(super) struct InstanceSource<S> {
    /// Decoder producing PCM samples.
    inner: S,

    /// Playback instance cancellation token.
    cancellation: CancellationToken,

    /// A prefetched source may start only if its predecessor did not fail.
    previous: Option<DecoderStatus>,
}

impl<S> InstanceSource<S> {
    /// Wraps one decoder with its instance token.
    pub(super) fn new(
        inner: S,
        cancellation: CancellationToken,
        previous: Option<DecoderStatus>,
    ) -> Self {
        Self {
            inner,
            cancellation,
            previous,
        }
    }
}

impl<S> Iterator for InstanceSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(previous) = self.previous.take()
            && matches!(previous.outcome(), Some(DecodeOutcome::Failed(_)))
        {
            self.cancellation.cancel();
        }
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

/// Opens the media with Symphonia while retaining its runtime decoding outcome.
pub(crate) fn build_decoder<R>(reader: R) -> crate::Result<Decoder>
where
    R: Read + Seek + Send + Sync + 'static,
{
    Decoder::new(reader).map_err(|source| crate::Error::Decode {
        source: Arc::new(source),
    })
}
