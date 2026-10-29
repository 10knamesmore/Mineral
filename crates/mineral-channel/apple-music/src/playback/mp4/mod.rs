//! Parses fragment timing and license initialization, and writes clear regular MP4 files.
//! Callers supply bytes and file handles; this module does not acquire media.

mod fragment;
mod pssh;
mod writer;

pub(super) use fragment::{AudioFragment, fragment_from_bytes};
pub(super) use pssh::widevine_pssh_payload;
pub(super) use writer::Mp4Writer;
