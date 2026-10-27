//! Source-neutral transfer progress shared by streaming readers and consumers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;

/// Contiguous media available from the beginning, reported by the producer.
/// Disjoint downloaded regions do not advance this boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BufferedPrefix {
    /// A byte-addressed producer that does not know the media timeline.
    Bytes {
        /// End of the contiguous available byte range, exclusive.
        end: u64,

        /// Full encoded byte length.
        total: u64,
    },

    /// A segmented producer that knows when its prepared audio can be played.
    Time {
        /// End of the contiguous prepared audio, relative to the track start.
        end: Duration,

        /// Full media duration.
        total: Duration,
    },
}

/// A point-in-time transfer progress snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransferSnapshot {
    /// Bytes made available by the producer.
    pub downloaded: u64,

    /// Total encoded byte length when known.
    pub total: Option<u64>,

    /// Producer-reported contiguous availability; None means not yet known.
    pub buffered: Option<BufferedPrefix>,

    /// Whether the producer reached its terminal completion state.
    pub complete: bool,
}

/// Cloneable progress handle for a bounded media producer.
#[derive(Clone, Default)]
pub struct TransferState {
    /// Shared mutable transfer facts.
    inner: Arc<TransferInner>,
}

/// Byte accounting and producer-reported contiguous availability.
#[derive(Default)]
struct TransferInner {
    /// Bytes made available by the producer.
    downloaded: AtomicU64,

    /// Total encoded byte length when known.
    total: Option<u64>,

    buffered: Mutex<Option<BufferedPrefix>>,

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
                buffered: Mutex::new(None),
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
            buffered: *self.inner.buffered.lock(),
            complete: self.inner.complete.load(Ordering::Acquire),
        }
    }

    /// Reports the number of bytes actually made available by the provider's producer.
    ///
    /// # Params:
    ///   - `downloaded`: Available byte count, rather than a reader's seek position.
    pub fn set_downloaded(&self, downloaded: u64) {
        self.inner.downloaded.store(downloaded, Ordering::Release);
    }

    /// Adds newly available, non-overlapping bytes from concurrent producer tasks.
    pub fn add_downloaded(&self, bytes: u64) {
        self.inner.downloaded.fetch_add(bytes, Ordering::AcqRel);
    }

    /// Publishes contiguous availability independently of total downloaded bytes.
    pub fn set_buffered(&self, buffered: BufferedPrefix) {
        *self.inner.buffered.lock() = Some(buffered);
    }

    /// Reports that the provider's producer has made all encoded media available.
    pub fn mark_complete(&self) {
        self.inner.complete.store(true, Ordering::Release);
    }
}
