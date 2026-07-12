//! 结构态存储。

pub(crate) mod schema;

mod envelope;
mod namespace;
pub(crate) mod rows;
mod session;
mod shelf;
mod song_kv;
mod time;

pub use namespace::{HistoryEntry, NamespaceStore, PlaylistCacheEntry, SongStats};
pub use session::{SessionSnapshot, SessionStore};
pub use shelf::{ShelfFileRow, ShelfStore};
pub use song_kv::RESERVED_KEYS;
