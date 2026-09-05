//! 结构态存储。

pub(crate) mod schema;

mod envelope;
mod namespace;
pub(crate) mod rows;
mod session;
mod song_kv;
mod song_meta;
pub(crate) mod time;

pub use namespace::{CachedPlaylistEntry, NamespaceStore, PlaylistCacheEntry};
pub use session::{SessionSnapshot, SessionStore};
pub use song_kv::RESERVED_KEYS;
