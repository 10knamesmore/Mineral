//! Catalog and personal-library reads mapped into source-neutral models.
//! Raw Apple resources stay inside this module; playback receives only a song identity.

mod albums;
mod artists;
mod client;
mod library;
mod lyrics;
mod mapping;
mod resources;
mod search;
mod songs;
mod wire;

pub(crate) use client::MetadataClient;
pub(crate) use songs::SongIdentity;
