//! Failures at the public server lifecycle boundary.

/// Failure while assembling the local play statistics response.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SongStatsError {
    /// Play statistics could not be read.
    #[error("play statistics unavailable")]
    Stats(#[from] mineral_stats::Error),

    /// The favorite status could not be read.
    #[error("favorite status unavailable")]
    Favorite(#[from] mineral_persist::Error),
}

/// Server startup, media integration, or IPC accept failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The audio engine could not start.
    #[error("audio engine startup failed")]
    Audio(#[from] mineral_audio::Error),

    /// The system media integration could not start.
    #[error("system media service startup failed")]
    Media(#[from] mineral_media::Error),

    /// The IPC listener could not accept a connection.
    #[error("IPC listener accept failed")]
    Accept(#[from] std::io::Error),
}
