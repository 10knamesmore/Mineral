//! Built-in preparation for directly accessible remote and local media.

mod buffer;
mod capture;
mod prepared;
mod reader;
mod remote;

pub use capture::CaptureError;
pub use prepared::DirectPreparedPlayback;
pub use remote::DownloadError;
