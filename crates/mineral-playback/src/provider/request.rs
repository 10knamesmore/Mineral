//! Source-neutral playback resource request.

use mineral_model::{BitRate, SongId};

/// The domain inputs required to resolve one playback resource.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackRequest {
    /// Requested song identity.
    song_id: SongId,

    /// Requested playback quality.
    quality: BitRate,
}

impl PlaybackRequest {
    /// Creates a playback resource request.
    ///
    /// # Params:
    ///   - `song_id`: Requested song identity.
    ///   - `quality`: Requested playback quality.
    #[must_use]
    pub fn new(song_id: SongId, quality: BitRate) -> Self {
        Self { song_id, quality }
    }

    /// Returns the requested song identity.
    #[must_use]
    pub fn song_id(&self) -> &SongId {
        &self.song_id
    }

    /// Returns the requested playback quality.
    #[must_use]
    pub fn quality(&self) -> BitRate {
        self.quality
    }
}
