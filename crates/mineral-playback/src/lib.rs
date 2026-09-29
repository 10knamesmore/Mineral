//! Source-neutral playback resource resolution and media preparation contracts.

mod direct;
mod error;
mod media;
mod provider;
mod registry;

pub use direct::DirectPreparedPlayback;
pub use error::Error;
/// Result of a playback operation.
pub type Result<T> = std::result::Result<T, Error>;
pub use media::{
    CaptureReceipt, CaptureTarget, CapturedMedia, MediaReader, OpenOptions, OpenedMedia,
    SeekSupport, TransferSnapshot, TransferState,
};
pub use mineral_model::{DirectLocator, DirectMedia, RemoteLocator};
pub use provider::{PlaybackProvider, PlaybackRequest, PreparedPlayback};
pub use registry::PlaybackRegistry;
