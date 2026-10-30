use serde::{Deserialize, Serialize};

use crate::{AlbumId, PlaylistId};

/// Collection lists are refreshed independently; their details are loaded on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CollectionCategory {
    /// User playlists across sources.
    Playlists,

    /// Saved albums across sources.
    Albums,
}

/// A refresh never changes queue contents or favorite membership.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RefreshScope {
    /// Reload this list and invalidate all of its detail reads.
    Collections(CollectionCategory),

    /// Reload the complete parent playlist.
    Playlist(PlaylistId),

    /// Reload the complete parent album.
    Album(AlbumId),
}

/// Controls reads, independently of playlist completeness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePolicy {
    /// Source adapters may reuse their normal caches.
    UseCache,

    /// Fetch original source metadata without a stale fallback; writes remain enabled.
    Reload,
}
