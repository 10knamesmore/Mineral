//! Mono PCM copied from rendered frames for the spectrum consumer.

use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use ringbuf::HeapProd;
use ringbuf::traits::Producer;

/// The output callback owns the producer through the shared playback state.
pub(crate) struct TapWriter {
    producer: HeapProd<f32>,

    sample_rate: Arc<AtomicU32>,

    pushed: Arc<AtomicU64>,
}

impl TapWriter {
    pub(crate) fn new(
        producer: HeapProd<f32>,
        sample_rate: Arc<AtomicU32>,
        pushed: Arc<AtomicU64>,
    ) -> Self {
        Self {
            producer,
            sample_rate,
            pushed,
        }
    }

    pub(crate) fn set_sample_rate(&self, rate: NonZeroU32) {
        self.sample_rate.store(rate.get(), Ordering::Relaxed);
    }

    /// A full visualization buffer drops samples without delaying audio output.
    pub(crate) fn push(&mut self, mono: f32) {
        self.pushed.fetch_add(1, Ordering::Relaxed);
        let _ = self.producer.try_push(mono);
    }
}
