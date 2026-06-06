//! 结构态存储。

pub(crate) mod schema;

mod namespace;
mod rows;
mod session;
mod song_kv;
mod time;

pub use namespace::{HistoryEntry, NamespaceStore, PlaylistCacheEntry, SongStats};
pub use session::{SessionSnapshot, SessionStore};
pub use song_kv::RESERVED_KEYS;
