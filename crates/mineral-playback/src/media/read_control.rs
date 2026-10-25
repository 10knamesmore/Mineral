//! Interruptible decoder read demand within one playback lifetime.

use std::sync::Arc;

use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

/// Shared read demand between a provider's reader and the decoder worker.
///
/// The worker calls [`Self::begin_read`] before each unit of decoder work. A seek
/// calls [`Self::interrupt`], leaving demand cancelled until the worker starts
/// replacement work. Cancelling the root token cancels all read demand.
#[derive(Clone)]
pub struct ReadControl {
    inner: Arc<ReadDemand>,
}

struct ReadDemand {
    root: CancellationToken,

    active: Mutex<CancellationToken>,
}

impl ReadControl {
    /// Creates active read demand governed by the playback lifetime token.
    ///
    /// # Params:
    ///   - `root`: Token governing the reader lifetime.
    #[must_use]
    pub fn new(root: CancellationToken) -> Self {
        let active = Mutex::new(root.child_token());
        Self {
            inner: Arc::new(ReadDemand { root, active }),
        }
    }

    /// Returns the current demand token for a provider's read request.
    ///
    /// After interruption, requests receive cancelled demand until [`Self::begin_read`]
    /// installs a new token. Previously returned tokens remain cancelled.
    #[must_use]
    pub fn request_cancellation(&self) -> CancellationToken {
        self.inner.active.lock().clone()
    }

    /// Cancels the previous demand and installs a fresh child of the root token.
    ///
    /// Call before replacement decoder work, after obsolete work has returned.
    /// A cancelled root also leaves the new demand cancelled.
    pub fn begin_read(&self) {
        let mut active = self.inner.active.lock();
        active.cancel();
        *active = self.inner.root.child_token();
    }

    /// Cancels active demand without renewing it.
    ///
    /// Reads remain interrupted until the worker calls [`Self::begin_read`].
    pub fn interrupt(&self) {
        self.inner.active.lock().cancel();
    }
}
