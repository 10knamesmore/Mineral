//! Lifecycle state for one gapless prefetch attempt.

use mineral_model::{DirectMedia, PlaybackMediaInfo, Song, SongId};
use mineral_protocol::PlaybackOrigin;

use crate::download::Capturing;
use crate::playback_instance::{PlaybackInstanceId, PlaybackSlot};

/// Media and accounting facts already armed behind the current decoder.
pub(crate) struct Queued {
    /// Prefetched song.
    pub(crate) song: Song,

    /// Final facts of the prefetched decoder input.
    pub(crate) media_info: PlaybackMediaInfo,

    /// Optional direct access associated with the prefetched media.
    pub(crate) direct_media: Option<DirectMedia>,

    /// Cache, download-library, or provider origin promoted with the song.
    pub(crate) origin: PlaybackOrigin,

    /// Post-preparation capture promoted with the song when enabled.
    pub(crate) capturing: Option<Capturing>,
}

/// Exclusive lifecycle of the next-song preparation attempt.
#[derive(Default)]
pub(crate) enum PrefetchState {
    /// No provider work or decoder is reserved for the next song.
    #[default]
    Idle,

    /// Resolve, hook, or open is in progress for this ownership slot.
    Opening(PlaybackSlot),

    /// The same slot has handed an opened decoder to audio and awaits promotion.
    Armed {
        /// Ownership and cancellation root retained through promotion.
        slot: PlaybackSlot,

        /// Media and accounting facts paired with the armed decoder.
        queued: Box<Queued>,
    },
}

impl PrefetchState {
    /// Returns whether an opened decoder is armed behind the current song.
    pub(crate) fn is_armed(&self) -> bool {
        matches!(self, Self::Armed { .. })
    }

    /// Returns the active ownership slot for opening or armed media.
    pub(crate) fn slot(&self) -> Option<&PlaybackSlot> {
        match self {
            Self::Idle => None,
            Self::Opening(slot) | Self::Armed { slot, .. } => Some(slot),
        }
    }

    /// Returns the song targeted by the active attempt.
    pub(crate) fn song_id(&self) -> Option<&SongId> {
        self.slot().map(|slot| &slot.song_id)
    }

    /// Returns mutable armed media facts.
    pub(crate) fn queued_mut(&mut self) -> Option<&mut Queued> {
        match self {
            Self::Armed { queued, .. } => Some(queued),
            Self::Idle | Self::Opening(_) => None,
        }
    }

    /// Replaces any active attempt with a new opening slot and cancels the displaced slot.
    ///
    /// # Params:
    ///   - `slot`: Ownership of the new prefetch attempt.
    pub(crate) fn replace_opening(&mut self, slot: PlaybackSlot) {
        drop(self.invalidate());
        *self = Self::Opening(slot);
    }

    /// Takes a still-matching opening slot before handing its decoder to audio.
    ///
    /// # Params:
    ///   - `instance_id`: Expected process-local attempt identity.
    ///   - `song_id`: Expected source-qualified song identity.
    pub(crate) fn take_opening(
        &mut self,
        instance_id: PlaybackInstanceId,
        song_id: &SongId,
    ) -> Option<PlaybackSlot> {
        if !matches!(self, Self::Opening(slot) if slot.matches(instance_id, song_id)) {
            return None;
        }
        let Self::Opening(slot) = std::mem::take(self) else {
            return None;
        };
        Some(slot)
    }

    /// Stores opened media with the slot that produced it.
    ///
    /// # Params:
    ///   - `slot`: Ownership retained from the opening state.
    ///   - `queued`: Opened media facts handed to audio.
    pub(crate) fn arm(&mut self, slot: PlaybackSlot, queued: Queued) {
        *self = Self::Armed {
            slot,
            queued: Box::new(queued),
        };
    }

    /// Takes an armed slot and its paired media facts for gapless promotion.
    pub(crate) fn take_armed(&mut self) -> Option<(PlaybackSlot, Queued)> {
        let Self::Armed { slot, queued } = std::mem::take(self) else {
            return None;
        };
        Some((slot, *queued))
    }

    /// Cancels the active slot and returns armed media facts for capture cleanup.
    pub(crate) fn invalidate(&mut self) -> Option<Queued> {
        match std::mem::take(self) {
            Self::Idle => None,
            Self::Opening(slot) => {
                slot.cancel();
                None
            }
            Self::Armed { slot, queued } => {
                slot.cancel();
                Some(*queued)
            }
        }
    }
}
