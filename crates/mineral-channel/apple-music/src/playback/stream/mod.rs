//! In-memory segments serve progressive AAC packets and complete-file consumers.
//! Readers own cancellation guards; capture and acquisition stop when those readers are dropped.

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
