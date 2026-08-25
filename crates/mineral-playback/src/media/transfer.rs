//! Source-neutral transfer progress shared by streaming readers and consumers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// A point-in-time transfer progress snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransferSnapshot {
    /// Bytes made available by the producer.
    pub downloaded: u64,

    /// Total encoded byte length when known.
    pub total: Option<u64>,

    /// Whether the producer reached its terminal completion state.
    pub complete: bool,
}

/// Cloneable progress handle for a bounded media producer.
#[derive(Clone, Default)]
pub struct TransferState {
    /// Shared mutable transfer facts.
    inner: Arc<TransferInner>,
}

/// Atomic transfer counters and immutable optional total length.
#[derive(Default)]
struct TransferInner {
    /// Bytes made available by the producer.
    downloaded: AtomicU64,

    /// Total encoded byte length when known.
    total: Option<u64>,

    /// Whether the producer reached its terminal completion state.
    complete: AtomicBool,
}

impl TransferState {
    /// Creates a transfer state with an optional known total length.
    #[must_use]
    pub fn new(total: Option<u64>) -> Self {
        Self {
            inner: Arc::new(TransferInner {
                downloaded: AtomicU64::new(0),
                total,
                complete: AtomicBool::new(false),
            }),
        }
    }

    /// Returns the latest transfer facts.
    #[must_use]
    pub fn snapshot(&self) -> TransferSnapshot {
        TransferSnapshot {
            downloaded: self.inner.downloaded.load(Ordering::Acquire),
            total: self.inner.total,
            complete: self.inner.complete.load(Ordering::Acquire),
        }
    }

    /// Updates the number of available bytes.
    pub(crate) fn set_downloaded(&self, downloaded: u64) {
        self.inner.downloaded.store(downloaded, Ordering::Release);
    }

    /// Marks the producer as complete.
    pub(crate) fn mark_complete(&self) {
        self.inner.complete.store(true, Ordering::Release);
    }
}
