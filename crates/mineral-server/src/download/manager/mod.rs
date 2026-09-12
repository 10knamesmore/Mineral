//! Session-only owner for flat Song download lifecycles.

mod admission;
mod completion;
mod lifecycle;
mod scheduler;
mod state;

#[cfg(test)]
mod tests;

pub(crate) use lifecycle::{DownloadManager, DownloadRuntime};
