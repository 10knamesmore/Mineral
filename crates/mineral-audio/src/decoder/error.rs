use std::io;
use std::num::TryFromIntError;
use std::time::Duration;

use symphonia::core::errors::Error as SymphoniaError;
use thiserror::Error;

/// Failures while opening, decoding, or seeking encoded audio.
#[derive(Debug, Error)]
pub enum DecoderError {
    #[error("inspect encoded reader position")]
    ReaderPosition(#[source] io::Error),

    #[error("inspect encoded reader length")]
    ReaderLength(#[source] io::Error),

    #[error("restore encoded reader position")]
    RestoreReader(#[source] io::Error),

    #[error("probe encoded media")]
    Probe(#[source] SymphoniaError),

    #[error("media has no playable audio track")]
    NoAudioTrack,

    #[error("create audio codec")]
    CreateCodec(#[source] SymphoniaError),

    #[error("media contains no decodable audio frames")]
    NoAudioFrames,

    #[error("read encoded audio packet")]
    ReadPacket(#[source] SymphoniaError),

    #[error("decode audio packet")]
    DecodePacket(#[source] SymphoniaError),

    #[error("decoded audio has invalid channel count {channels}")]
    InvalidChannels { channels: usize },

    #[error("decoded audio has zero sample rate")]
    InvalidSampleRate,

    #[error("audio timing exceeds the supported range")]
    TimingOverflow,

    #[error("convert audio timing")]
    TimingConversion(#[source] TryFromIntError),

    #[error("seek encoded audio")]
    Seek(#[source] SymphoniaError),

    #[error("audio track has no time base for seeking")]
    SeekUnsupported,

    #[error("audio ended before seek position {position:?}")]
    SeekPastEnd { position: Duration },
}
