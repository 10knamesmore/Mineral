use mineral_playback::{AudioInput, AudioPacketSource};
use symphonia::core::errors::Result as FormatResult;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{
    FormatOptions, FormatReader, SeekMode, SeekTo, SeekedTo, Track, TrackType,
};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::packet::Packet;
use symphonia::default::get_probe;

use super::error::DecoderError;
use super::reader::EncodedReader;

/// Ordinary container inputs and source-owned segmented inputs share the same codec path.
pub(super) fn open(input: AudioInput) -> Result<Box<dyn AudioPacketSource>, DecoderError> {
    match input {
        AudioInput::Packets(packets) => Ok(packets),
        AudioInput::Bytes { reader, byte_len } => {
            let reader = EncodedReader::new(reader, byte_len);
            let stream = MediaSourceStream::new(Box::new(reader), Default::default());
            let format = get_probe()
                .probe(
                    &Hint::new(),
                    stream,
                    FormatOptions::default(),
                    MetadataOptions::default(),
                )
                .map_err(DecoderError::Probe)?;
            let track = format
                .default_track(TrackType::Audio)
                .ok_or(DecoderError::NoAudioTrack)?
                .clone();
            Ok(Box::new(ContainerPackets { format, track }))
        }
    }
}

struct ContainerPackets {
    format: Box<dyn FormatReader>,

    track: Track,
}

impl AudioPacketSource for ContainerPackets {
    fn track(&self) -> &Track {
        &self.track
    }

    fn next_packet(&mut self) -> FormatResult<Option<Packet>> {
        self.format.next_packet()
    }

    fn seek(&mut self, mode: SeekMode, to: SeekTo) -> FormatResult<SeekedTo> {
        self.format.seek(mode, to)
    }
}
