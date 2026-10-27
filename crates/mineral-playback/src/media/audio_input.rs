use symphonia_core::errors::Result;
use symphonia_core::formats::{SeekMode, SeekTo, SeekedTo, Track};
use symphonia_core::packet::Packet;

use super::MediaReader;

/// Compressed packets from one audio track, prepared by its source.
///
/// Packet timestamps and seek results use the full track's timebase, not a segment-local clock.
/// The track's codec configuration remains valid across packets. Reads may wait for acquisition;
/// sources attach a [`super::ReadControl`] to interrupt obsolete read or seek demand.
pub trait AudioPacketSource: Send + Sync {
    /// Describes the audio track and its codec initialization data.
    /// Duration may become more precise as segment metadata arrives.
    fn track(&self) -> &Track;

    /// Returns the next compressed packet; None means the end of the full track.
    fn next_packet(&mut self) -> Result<Option<Packet>>;

    /// Positions subsequent packet reads on the full track's timeline.
    fn seek(&mut self, mode: SeekMode, to: SeekTo) -> Result<SeekedTo>;
}

/// A container byte stream or source-demultiplexed compressed audio.
/// Both forms are decoded by the same audio engine.
pub enum AudioInput {
    /// The audio engine probes and demultiplexes the container.
    Bytes(Box<dyn MediaReader>),

    /// The source owns segment selection and container demultiplexing.
    Packets(Box<dyn AudioPacketSource>),
}
