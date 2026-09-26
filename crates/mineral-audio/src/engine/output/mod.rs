//! Audio output device discovery, stream ownership, and queue handoff.

mod devices;
mod manager;
mod source;
mod stream;
mod watchdog;

pub(super) use manager::Output;
