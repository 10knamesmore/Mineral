//! Session-only owner for flat Song download lifecycles.

mod admission;
mod completion;
mod lifecycle;
mod scheduler;
mod state;

pub(crate) use lifecycle::{DownloadManager, DownloadRuntime, StopError};
