mod decode;
mod error;
mod reader;

pub(crate) use decode::{DecodedAudio, Decoder, PcmFormat};
pub use error::DecoderError;
