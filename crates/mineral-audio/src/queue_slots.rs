//! Two-slot audio queue accounting for current and gapless-prefetched media.

use mineral_playback::TransferState;
use tokio_util::sync::CancellationToken;

use crate::bps::Bps;
use crate::capture::CaptureState;

/// Audio-engine accounting for one appended decoder.
#[derive(Clone, Default)]
pub(crate) struct Slot {
    /// Decoder-reported duration in milliseconds.
    pub(crate) duration_ms: Option<u64>,

    /// Decoder sample rate in hertz.
    pub(crate) sample_rate: u32,

    /// Producer transfer progress for streaming media.
    pub(crate) transfer: Option<TransferState>,

    /// Post-preparation capture completion when capture is enabled.
    pub(crate) capture: Option<CaptureState>,

    /// Cancellation token governing this decoder and its producer.
    pub(crate) cancellation: Option<CancellationToken>,

    /// Whether a decoder occupies this slot.
    pub(crate) occupied: bool,
}

impl Slot {
    /// Cancels the decoder and producer owned by this slot.
    pub(crate) fn cancel(&self) {
        if let Some(cancellation) = &self.cancellation {
            cancellation.cancel();
        }
    }

    /// Returns source-neutral buffering and capture completion facts.
    fn progress(&self) -> (Bps, bool) {
        if !self.occupied {
            return (Bps::ZERO, false);
        }
        let buffered = match &self.transfer {
            None => Bps::FULL,
            Some(transfer) => {
                let snapshot = transfer.snapshot();
                if snapshot.complete {
                    Bps::FULL
                } else {
                    snapshot
                        .total
                        .map_or(Bps::ZERO, |total| Bps::ratio(snapshot.downloaded, total))
                }
            }
        };
        let captured = self.capture.as_ref().is_some_and(CaptureState::complete);
        (buffered, captured)
    }
}

/// A boundary observed from the decoder queue length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Boundary {
    /// No decoder boundary occurred.
    None,

    /// Current media ended and the armed next decoder continued gaplessly.
    Gapless,

    /// Current media ended without an armed next decoder.
    EndOfQueue,
}

/// Current and prefetched decoder slots observed across audio ticks.
#[derive(Default)]
pub(crate) struct PlayHead {
    /// Decoder currently producing audio.
    pub(crate) cur: Slot,

    /// Decoder armed behind the current source.
    pub(crate) next: Slot,

    /// Monotonic count of naturally completed tracks.
    pub(crate) finished_seq: u64,

    /// Monotonic token changed on cut-over and gapless promotion.
    pub(crate) track_token: u64,
}

impl PlayHead {
    /// Replaces current media and cancels both prior slots.
    ///
    /// # Params:
    ///   - `slot`: Newly appended current decoder.
    pub(crate) fn start(&mut self, slot: Slot) {
        self.cur.cancel();
        self.next.cancel();
        self.cur = slot;
        self.next = Slot::default();
        self.track_token = self.track_token.saturating_add(1);
    }

    /// Replaces the armed next decoder.
    ///
    /// # Params:
    ///   - `slot`: Newly appended gapless decoder.
    pub(crate) fn arm_next(&mut self, slot: Slot) {
        self.next.cancel();
        self.next = slot;
    }

    /// Cancels and disarms the prefetched decoder.
    pub(crate) fn clear_next(&mut self) {
        self.next.cancel();
        self.next = Slot::default();
    }

    /// Cancels and clears both decoder slots.
    pub(crate) fn stop(&mut self) {
        self.cur.cancel();
        self.next.cancel();
        self.cur = Slot::default();
        self.next = Slot::default();
    }

    /// Produces fields copied into the public audio snapshot.
    pub(crate) fn snapshot_fields(&self) -> GaplessFields {
        let (buffered_bps, download_complete) = self.cur.progress();
        let (next_buffered_bps, next_download_complete) = self.next.progress();
        GaplessFields {
            current_track_token: self.track_token,
            track_finished_seq: self.finished_seq,
            duration_ms: self.cur.occupied.then_some(self.cur.duration_ms).flatten(),
            buffered_bps,
            download_complete,
            next_duration_ms: self
                .next
                .occupied
                .then_some(self.next.duration_ms)
                .flatten(),
            next_buffered_bps,
            next_ready: self.next.occupied,
            next_download_complete,
        }
    }

    /// Observes decoder queue length and advances the two-slot state machine.
    ///
    /// # Params:
    ///   - `len`: Number of sources remaining in the audio queue.
    pub(crate) fn observe(&mut self, len: usize) -> Boundary {
        let armed = usize::from(self.cur.occupied) + usize::from(self.next.occupied);
        if len >= armed {
            return Boundary::None;
        }
        self.finished_seq = self.finished_seq.saturating_add(1);
        self.cur.cancel();
        if self.next.occupied {
            self.cur = std::mem::take(&mut self.next);
            self.track_token = self.track_token.saturating_add(1);
            Boundary::Gapless
        } else {
            self.cur = Slot::default();
            Boundary::EndOfQueue
        }
    }
}

/// Snapshot fields derived from the current two-slot state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GaplessFields {
    /// Current track identity token.
    pub(crate) current_track_token: u64,

    /// Monotonic natural-finish sequence.
    pub(crate) track_finished_seq: u64,

    /// Current decoder duration.
    pub(crate) duration_ms: Option<u64>,

    /// Current producer buffer ratio.
    pub(crate) buffered_bps: Bps,

    /// Whether the current post-preparation capture is complete.
    pub(crate) download_complete: bool,

    /// Next decoder duration.
    pub(crate) next_duration_ms: Option<u64>,

    /// Next producer buffer ratio.
    pub(crate) next_buffered_bps: Bps,

    /// Whether the next decoder is armed.
    pub(crate) next_ready: bool,

    /// Whether the next post-preparation capture is complete.
    pub(crate) next_download_complete: bool,
}

#[cfg(test)]
mod tests {
    use super::{Boundary, Bps, PlayHead, Slot};

    /// Creates an occupied slot with a deterministic duration.
    fn slot(duration_ms: u64) -> Slot {
        Slot {
            duration_ms: Some(duration_ms),
            occupied: true,
            ..Slot::default()
        }
    }

    /// Starting a decoder occupies current and increments identity.
    #[test]
    fn start_occupies_current_and_bumps_token() {
        let mut head = PlayHead::default();
        head.start(slot(1_000));
        assert_eq!(head.track_token, 1);
        assert!(head.cur.occupied);
        assert!(!head.next.occupied);
        assert_eq!(head.cur.duration_ms, Some(1_000));
    }

    /// A natural end without next reports end-of-queue.
    #[test]
    fn single_track_end_reports_end_of_queue() {
        let mut head = PlayHead::default();
        head.start(slot(1_000));
        assert_eq!(head.observe(1), Boundary::None);
        assert_eq!(head.observe(0), Boundary::EndOfQueue);
        assert_eq!(head.finished_seq, 1);
        assert!(!head.cur.occupied);
    }

    /// An armed next decoder rotates into current without a second start.
    #[test]
    fn gapless_boundary_rotates_next() {
        let mut head = PlayHead::default();
        head.start(slot(1_000));
        head.arm_next(slot(2_000));
        assert_eq!(head.observe(2), Boundary::None);
        assert_eq!(head.observe(1), Boundary::Gapless);
        assert_eq!(head.finished_seq, 1);
        assert_eq!(head.track_token, 2);
        assert_eq!(head.cur.duration_ms, Some(2_000));
        assert!(!head.next.occupied);
    }

    /// Unoccupied slots do not leak stale progress into snapshots.
    #[test]
    fn empty_slots_have_zero_progress() {
        let fields = PlayHead::default().snapshot_fields();
        assert_eq!(fields.buffered_bps, Bps::ZERO);
        assert!(!fields.download_complete);
        assert_eq!(fields.next_buffered_bps, Bps::ZERO);
        assert!(!fields.next_ready);
    }
}
