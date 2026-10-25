//! Indexed AAC media backed by independently acquired and decrypted CENC segments.

mod cenc;
mod error;
mod index;
mod license;
mod open;
mod reader;
mod store;
mod transport;

pub(crate) use open::{PlaybackIdentity, open};
