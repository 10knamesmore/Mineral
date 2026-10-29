//! Apple Music accounts, catalog/library metadata and progressive audio playback.
//!
//! `channel` adapts the public channel/playback traits. `account` owns authentication,
//! `metadata` maps Apple resources into shared models, and `playback` prepares media.

mod account;
mod channel;
pub mod cli;
mod config;
mod error;
mod metadata;
mod playback;

pub use account::{StoredAppleMusicAuth, load_stored};
pub use channel::AppleMusicChannel;
pub use config::{AlbumCacheConfig, AppleMusicConfig};
pub use error::Error;

/// Result of an Apple Music source operation.
pub type Result<T> = std::result::Result<T, Error>;
