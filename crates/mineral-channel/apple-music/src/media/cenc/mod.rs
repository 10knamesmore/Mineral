//! Single-track ISO CENC AAC sample extraction with RustCrypto AES-CTR.
//! Content keys are supplied by the caller.

pub(crate) use self::error::Error;
pub(crate) use self::initialization::AudioInitialization;

mod auxiliary;
mod binary;
mod error;
mod fragment;
mod initialization;
