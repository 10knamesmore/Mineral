//! TUI preferences, cursor positions and cover-cache schema.

mod entity;
mod error;
mod migration;
mod store;

pub use error::{Error, Result};
pub(crate) use store::{TrackPosRow, TuiStore};
