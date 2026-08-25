//! Object-safe resolve and single-use prepared playback contracts.

use async_trait::async_trait;
use mineral_model::SourceKind;
use tokio_util::sync::CancellationToken;

use super::PlaybackRequest;
use crate::{DirectMedia, OpenOptions, OpenedMedia};

/// Resolves source identities into process-local prepared playback plans.
///
/// Implementations own source authentication and resource selection. They must not open or read
/// encoded media during `resolve`; media acquisition and preparation begin in `PreparedPlayback::open`.
#[async_trait]
pub trait PlaybackProvider: Send + Sync {
    /// Returns the source served by this provider.
    fn source(&self) -> SourceKind;

    /// Resolves a source-neutral request without opening media bytes.
    ///
    /// # Params:
    ///   - `request`: Song identity and requested quality.
    ///   - `cancellation`: Playback instance cancellation token.
    ///
    /// # Return:
    ///   A process-local single-use plan.
    async fn resolve(
        &self,
        request: PlaybackRequest,
        cancellation: CancellationToken,
    ) -> color_eyre::Result<Box<dyn PreparedPlayback>>;
}

/// A resolved, unopened, single-use playback plan.
///
/// Implementations may keep arbitrary source-private state. `open` consumes the boxed plan so safe
/// Rust cannot open the same plan twice.
#[async_trait]
pub trait PreparedPlayback: Send {
    /// Returns an optional direct access capability available before opening.
    fn direct_media(&self) -> Option<&DirectMedia>;

    /// Opens and prepares decoder-ready encoded media.
    ///
    /// # Params:
    ///   - `options`: Open-time cancellation and buffering controls.
    ///
    /// # Return:
    ///   Decoder-ready encoded media.
    async fn open(self: Box<Self>, options: OpenOptions) -> color_eyre::Result<OpenedMedia>;
}
