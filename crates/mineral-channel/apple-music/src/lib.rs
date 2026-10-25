//! Apple Music catalog, personal library and account authentication.

mod album;
mod api;
mod artist;
mod channel;
pub mod cli;
mod config;
mod credential;
mod error;
mod lyrics;
mod mapping;
mod media;
mod playback;
mod session;
mod wire;

pub use channel::AppleMusicChannel;
pub use config::{AlbumCacheConfig, AppleMusicConfig};
pub use credential::{StoredAppleMusicAuth, load_stored};
pub use error::Error;

/// Result of an Apple Music source operation.
pub type Result<T> = std::result::Result<T, Error>;
