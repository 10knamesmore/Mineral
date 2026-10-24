//! Device discovery and CPAL stream replacement over shared prepared PCM.

mod devices;
mod manager;
mod stream;
mod watchdog;

pub(super) use manager::Output;
