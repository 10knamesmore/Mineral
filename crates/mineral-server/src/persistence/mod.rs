//! Daemon-owned functional data, schema and database access.

mod channel_store;
mod db;
mod entity;
mod error;
mod migration;
mod store;

pub use db::{NamespaceStore, SessionSnapshot};
pub use error::{Error, Result};
pub use store::{PlaylistCacheStats, ServerStore};

#[cfg(test)]
mod isolation_tests;

#[cfg(test)]
mod aggregate_channel_tests;
