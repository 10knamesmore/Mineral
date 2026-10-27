//! Request and device settings supplied by daemon configuration.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Duration;

/// Defaults belong to daemon-default.lua; device identity is only needed for playback.
#[derive(Clone, Debug, typed_builder::TypedBuilder, derive_getters::Getters)]
pub struct AppleMusicConfig {
    /// Proxy used for Apple requests.
    proxy: Option<String>,

    /// Request timeout, in seconds.
    timeout_secs: u64,

    /// Maximum age of a cached website developer token; JWT expiry can shorten it.
    developer_token_cache_ttl: Duration,

    /// Apple lyrics translation language tag; None omits l[lyrics].
    lyrics_translation_language: Option<String>,

    /// Apple lyrics romanization script tag; None omits l[script].
    lyrics_romanization_script: Option<String>,

    /// Persistent album-detail cache lifetime, supplied by the daemon.
    album_cache: AlbumCacheConfig,

    /// Playback device file; None selects the bundled identity. File errors are returned.
    device_file: Option<PathBuf>,

    /// Maximum concurrent background segment acquisitions.
    media_concurrency: NonZeroUsize,

    /// Contiguous playable duration required before the first playback handoff.
    startup_buffer: Duration,

    /// Initial byte range for a segment header needed before acquiring its audio.
    fragment_header_prefetch_bytes: NonZeroUsize,
}

/// Expiration is chosen after a successful fetch; cache reads do not extend it.
#[derive(Clone, Debug, typed_builder::TypedBuilder, derive_getters::Getters)]
pub struct AlbumCacheConfig {
    /// Base lifetime in days.
    ttl_days: u32,

    /// Maximum lifetime adjustment in either direction, in days.
    ttl_jitter_days: u32,
}
