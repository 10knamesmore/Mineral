use std::io;
use std::num::TryFromIntError;

use mineral_playback::Error as PlaybackError;
use shiguredo_mp4::{Error as Mp4Error, mux::MuxError};
use thiserror::Error as ThisError;

use super::{cenc, license};

pub(super) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, ThisError)]
pub(crate) enum Error {
    #[error("Apple Music request failed")]
    Account(#[from] crate::Error),

    #[error("Apple Music playback was rejected")]
    PlaybackRejected,

    #[error("Apple Music did not supply a complete AAC rendition")]
    NoRendition,

    #[error("invalid Apple Music media field: {field}")]
    InvalidMedia { field: &'static str },

    #[error("unsupported Apple Music media layout: {layout}")]
    Unsupported { layout: &'static str },

    #[error("Apple Music media request failed")]
    Network(#[source] reqwest::Error),

    #[error("Apple Music CDN returned HTTP {status}")]
    Http { status: u16 },

    #[error("Apple Music CDN did not return the requested byte range")]
    ByteRange,

    #[error("invalid media response")]
    Response(#[from] serde_json::Error),

    #[error("invalid media URL")]
    Url(#[from] url::ParseError),

    #[error("invalid license encoding")]
    Base64(#[from] base64::DecodeError),

    #[error("license service rejected the request with status {status}")]
    LicenseRejected { status: i32 },

    #[error("license exchange failed")]
    License(#[from] license::Error),

    #[error("encrypted media preparation failed")]
    Cenc(#[from] cenc::Error),

    #[error("MP4 structure could not be read")]
    Mp4(#[from] Mp4Error),

    #[error("MP4 sample index could not be built")]
    Mux(#[from] MuxError),

    #[error("media size or timing exceeds the supported range")]
    Number(#[from] TryFromIntError),

    #[error("media size or timing overflow")]
    Overflow,

    #[error("prepared media I/O failed")]
    Io(#[from] io::Error),

    #[error("media capture worker failed")]
    CaptureTask(#[from] tokio::task::JoinError),

    #[error("media request cancelled")]
    Cancelled,

    // Direct opener errors may retain signed URLs, so they are not exposed as a source chain.
    #[error("could not open uploaded Apple Music media")]
    Upload { detail: PlaybackError },
}

impl Error {
    pub(super) fn network(error: reqwest::Error) -> Self {
        Self::Network(error.without_url())
    }
}

impl From<Error> for PlaybackError {
    fn from(error: Error) -> Self {
        match error {
            Error::Cancelled => Self::Cancelled,
            other => Self::Provider {
                source: Box::new(other),
            },
        }
    }
}
