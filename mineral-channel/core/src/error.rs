use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("network: {0}")]
    Network(String),

    #[error("api code {code}: {message}")]
    Api { code: i64, message: String },

    #[error("authentication required")]
    AuthRequired,

    #[error("rate limited")]
    RateLimited,

    #[error("not supported by this channel")]
    NotSupported,

    #[error("parse: {0}")]
    Parse(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
