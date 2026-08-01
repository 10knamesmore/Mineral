//! 结构态存储。

pub(crate) mod schema;

mod envelope;
mod namespace;
pub(crate) mod rows;
mod session;
mod song_kv;
pub(crate) mod time;

pub use namespace::{
    CachedPlaylistEntry, HistoryEntry, NamespaceStore, PlaylistCacheEntry, SongStats,
};
pub use session::{SessionSnapshot, SessionStore};
pub use song_kv::RESERVED_KEYS;
