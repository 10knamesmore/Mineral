//! Playback parameters copied from the daemon-supplied configuration without adding defaults.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Duration;

use crate::AppleMusicConfig;

/// Captured when a song is resolved and retained until its media opens.
pub(super) struct PlaybackSettings {
    /// None selects the bundled Widevine identity; errors reading a selected file propagate.
    pub(super) device_file: Option<PathBuf>,

    /// Maximum number of concurrently prepared media segments.
    pub(super) media_concurrency: NonZeroUsize,

    /// Exact contiguous audio duration required before handing media to the player.
    pub(super) startup_buffer: Duration,

    /// Initial range length in bytes when reading a fragment's header without its audio.
    pub(super) fragment_header_prefetch_bytes: NonZeroUsize,
}

impl From<&AppleMusicConfig> for PlaybackSettings {
    fn from(config: &AppleMusicConfig) -> Self {
        Self {
            device_file: config.device_file().clone(),
            media_concurrency: *config.media_concurrency(),
            startup_buffer: *config.startup_buffer(),
            fragment_header_prefetch_bytes: *config.fragment_header_prefetch_bytes(),
        }
    }
}
