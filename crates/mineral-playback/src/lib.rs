//! Source-neutral playback resource resolution and media preparation contracts.

mod direct;
mod media;
mod provider;
mod registry;

pub use direct::DirectPreparedPlayback;
pub use media::{
    CaptureReceipt, CaptureTarget, CapturedMedia, MediaReader, OpenOptions, OpenedMedia,
    SeekSupport, TransferSnapshot, TransferState,
};
pub use mineral_model::{DirectLocator, DirectMedia, RemoteLocator};
pub use provider::{PlaybackProvider, PlaybackRequest, PreparedPlayback};
pub use registry::PlaybackRegistry;
