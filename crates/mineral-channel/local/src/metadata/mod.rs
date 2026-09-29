//! Probe local audio metadata and normalize embedded lyrics for library scans.

mod file;
mod lyrics;
mod mp4_duration;

pub(crate) use file::Metadata;
pub(crate) use file::{cover, probe, supported};
pub(crate) use lyrics::{LyricsEncodingError, decode_sidecar};
