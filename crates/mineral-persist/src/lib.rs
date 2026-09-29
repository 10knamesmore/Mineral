//! Shared SQLite connection and file-cache mechanisms.
//! Database owners define and initialize their own tables.

mod cache_index;
mod error;
mod pool;

pub use cache_index::{CacheEntryStat, CacheIndex, CacheStats, Evicted};
pub use error::Error;
pub use pool::connect;
/// Storage mechanism operation result.
pub type Result<T> = std::result::Result<T, Error>;
