//! Classified failures without credentials or remote-response contents.

use std::io;

use inquire::InquireError;
use mineral_channel_core::store::StoreError;
use thiserror::Error as ThisError;

/// Authentication, transport, metadata and local credential failures.
#[derive(Debug, ThisError)]
pub enum Error {
    /// The source-scoped album cache could not be read or written.
    #[error("Apple Music album cache storage failed")]
    Storage(#[from] StoreError),

    /// A library artist has no catalog counterpart for catalog views.
    #[error("Apple Music library artist has no catalog counterpart")]
    NoCatalogArtist,

    /// Playlist reads support personal-library IDs, not catalog playlist IDs.
    #[error("Apple Music playlist reads require a personal-library playlist")]
    LibraryPlaylistRequired,

    /// Artist editorial notes could not be converted to plain text.
    #[error("invalid Apple Music artist editorial notes")]
    EditorialNotes(#[from] html2text::Error),

    /// Credentials are missing, expired or rejected by Apple.
    #[error("Apple Music login required; run mineral channel am login")]
    AuthRequired,

    /// Input must contain only the cookie value, not a Cookie header or assignment.
    #[error(
        "Paste only the media-user-token cookie value from music.apple.com, not its name or a Cookie header."
    )]
    InvalidTokenValue,

    /// Account validation rejected the credentials supplied to the login command.
    #[error(
        "Apple did not accept these credentials. Sign in at music.apple.com and copy a fresh media-user-token value."
    )]
    LoginRejected,

    /// The account response did not include subscription information.
    #[error(
        "Apple Music returned no subscription information. Check your subscription at music.apple.com."
    )]
    SubscriptionUnavailable,

    /// Apple returned an unsuccessful HTTP status.
    #[error("Apple Music HTTP status {status}")]
    Http {
        /// Response bodies are deliberately excluded.
        status: u16,
    },

    /// A remote request could not complete.
    #[error("Apple Music request failed")]
    Network(#[source] reqwest::Error),

    /// A typed response could not be decoded.
    #[error("invalid Apple Music response")]
    Parse(#[from] serde_json::Error),

    /// The lyrics resource contains malformed XML.
    #[error("invalid Apple Music TTML")]
    Ttml(#[from] roxmltree::Error),

    /// Credential JSON errors omit serde's message because it can contain token values.
    #[error("invalid Apple Music credential file at line {line}, column {column}")]
    CredentialFormat {
        /// Line reported by the JSON decoder.
        line: usize,

        /// Column reported by the JSON decoder.
        column: usize,
    },

    /// Website asset patterns could not be compiled.
    #[error("invalid Apple Music website pattern")]
    Pattern(#[from] regex::Error),

    /// A developer token has invalid encoding.
    #[error("invalid Apple Music payload encoding")]
    Base64(#[from] base64::DecodeError),

    /// A provider URL could not be parsed.
    #[error("invalid Apple Music URL")]
    Url(#[from] url::ParseError),

    /// A release date could not be parsed.
    #[error("invalid Apple Music release date")]
    Date(#[from] chrono::ParseError),

    /// A remote count exceeds the model's supported range.
    #[error("Apple Music number out of range")]
    Number(#[from] std::num::TryFromIntError),

    /// System time could not be used to check token expiry.
    #[error("invalid system clock")]
    Clock(#[from] std::time::SystemTimeError),

    /// A credential cannot be represented as an HTTP header.
    #[error("invalid Apple Music credential header")]
    Header(#[from] reqwest::header::InvalidHeaderValue),

    /// A required resource field was absent or invalid.
    #[error("invalid Apple Music {field}")]
    InvalidData {
        /// Field or resource that violated the expected shape.
        field: &'static str,
    },

    /// Local credential or terminal I/O failed.
    #[error("Apple Music local I/O failed")]
    Io(#[from] io::Error),

    /// The terminal token prompt could not complete.
    #[error("Apple Music token input failed")]
    Prompt(#[source] InquireError),

    /// Application paths could not be resolved.
    #[error(transparent)]
    Paths(#[from] mineral_paths::Error),

    /// The user cancelled an operation.
    #[error("Apple Music operation cancelled")]
    Cancelled,

    /// A blocking credential operation could not finish.
    #[error("Apple Music credential task failed")]
    Task(#[from] tokio::task::JoinError),
}

impl Error {
    /// URLs can contain private request parameters; exclude them from error chains.
    pub(crate) fn network(error: reqwest::Error) -> Self {
        Self::Network(error.without_url())
    }
}

impl From<Error> for mineral_channel_core::Error {
    fn from(error: Error) -> Self {
        match error {
            Error::NoCatalogArtist | Error::LibraryPlaylistRequired => Self::NotSupported,
            Error::EditorialNotes(source) => Self::Parse {
                source: Box::new(source),
            },
            Error::Storage(source) => Self::Storage {
                source: Box::new(source),
            },
            Error::AuthRequired
            | Error::InvalidTokenValue
            | Error::LoginRejected
            | Error::SubscriptionUnavailable => Self::AuthRequired,
            Error::Http { status: 404 } => Self::NotFound,
            Error::Http { status: 429 } => Self::RateLimited,
            Error::InvalidData { field } => Self::InvalidData { field },
            Error::Parse(source) => Self::Parse {
                source: Box::new(source),
            },
            Error::Ttml(source) => Self::Parse {
                source: Box::new(source),
            },
            other => Self::Network {
                source: Box::new(other),
            },
        }
    }
}

impl From<Error> for mineral_playback::Error {
    fn from(error: Error) -> Self {
        match error {
            Error::Cancelled => Self::Cancelled,
            other => Self::Provider {
                source: Box::new(other),
            },
        }
    }
}
