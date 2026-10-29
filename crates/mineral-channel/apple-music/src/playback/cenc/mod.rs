//! Single-track CENC/CBCS AAC sample extraction with RustCrypto AES-CTR/AES-CBC.
//! Content keys and resource byte offsets are supplied by the caller; this module
//! neither acquires media nor owns playback cancellation.

pub(crate) use self::error::Error;
pub(crate) use self::initialization::AudioInitialization;

mod auxiliary;
mod binary;
mod error;
mod fragment;
mod initialization;
