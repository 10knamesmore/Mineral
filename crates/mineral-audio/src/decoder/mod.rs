//! Symphonia decoding with explicit completion and failure results.

mod error;
mod reader;
mod source;

pub use error::DecoderError;
pub(crate) use source::{DecodeOutcome, Decoder, DecoderStatus};
