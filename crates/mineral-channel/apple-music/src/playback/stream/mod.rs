//! One segment store serves progressive AAC packets and complete-file consumers.
//! Consumers retain cancellation drop guards; acquisition workers retain only the store.

pub(super) use capture::capture;
pub(super) use packets::AudioPackets;
pub(super) use reader::CompleteFileReader;
pub(super) use segments::SegmentStore;

mod capture;
mod export;
mod packets;
mod reader;
mod segments;
mod timeline;
