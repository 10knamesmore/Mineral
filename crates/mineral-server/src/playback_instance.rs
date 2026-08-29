//! Process-local identity and cancellation for current and prefetched playback attempts.

use mineral_model::SongId;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Unique identity of one real current-play or prefetch attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PlaybackInstanceId(Uuid);

impl std::fmt::Display for PlaybackInstanceId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl PlaybackInstanceId {
    /// Creates a fresh process-local playback identity.
    pub(crate) fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// Active ownership slot for one playback attempt.
#[derive(Clone)]
pub(crate) struct PlaybackSlot {
    /// Unique attempt identity.
    pub(crate) instance_id: PlaybackInstanceId,

    /// Defensive domain identity paired with the attempt.
    pub(crate) song_id: SongId,

    /// Root cancellation token for resolve, hook, open, and reader work.
    pub(crate) cancellation: CancellationToken,
}

impl PlaybackSlot {
    /// Creates a fresh slot for a song.
    ///
    /// # Params:
    ///   - `song_id`: Song targeted by this attempt.
    pub(crate) fn new(song_id: SongId) -> Self {
        Self {
            instance_id: PlaybackInstanceId::new(),
            song_id,
            cancellation: CancellationToken::new(),
        }
    }

    /// Returns whether both attempt and song identities match.
    ///
    /// # Params:
    ///   - `instance_id`: Attempt identity to validate.
    ///   - `song_id`: Song identity to validate.
    pub(crate) fn matches(&self, instance_id: PlaybackInstanceId, song_id: &SongId) -> bool {
        self.instance_id == instance_id && self.song_id == *song_id
    }

    /// Cancels all work owned by this slot.
    pub(crate) fn cancel(&self) {
        self.cancellation.cancel();
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::{SongId, SourceKind};

    use super::PlaybackSlot;

    /// Two attempts for the same song still have different identities.
    #[test]
    fn same_song_attempts_are_distinct() {
        let song = SongId::new(SourceKind::NETEASE, "1");
        let first = PlaybackSlot::new(song.clone());
        let second = PlaybackSlot::new(song);
        assert_ne!(first.instance_id, second.instance_id);
    }
}
