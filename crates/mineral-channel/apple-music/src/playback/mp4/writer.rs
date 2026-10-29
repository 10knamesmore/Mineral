//! Writes clear sample payloads and their original timing into a regular MP4 container.

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::num::NonZeroU32;

use shiguredo_mp4::boxes::{FreeBox, MdatBox, SampleEntry};
use shiguredo_mp4::mux::{Mp4FileMuxer, Sample};
use shiguredo_mp4::{BoxHeader, BoxSize, Decode, Encode, TrackKind};

use super::fragment::AudioFragment;
use crate::playback::cenc::AudioInitialization;
use crate::playback::error::{Error, Result};

/// One regular-MP4 builder serves both segment demuxing and complete file export.
pub(in crate::playback) struct Mp4Writer {
    muxer: Mp4FileMuxer,

    first_entry: Option<SampleEntry>,

    position: u64,

    timescale: NonZeroU32,
}

impl Mp4Writer {
    pub(in crate::playback) fn new(
        output: &mut impl Write,
        initialization: &AudioInitialization,
    ) -> Result<Self> {
        let muxer = Mp4FileMuxer::new()?;
        output.write_all(muxer.initial_boxes_bytes())?;
        Ok(Self {
            position: u64::try_from(muxer.initial_boxes_bytes().len())?,
            muxer,
            first_entry: Some(initialization.clear_sample_entry().clone()),
            timescale: initialization.timescale(),
        })
    }

    pub(in crate::playback) fn position(&self) -> u64 {
        self.position
    }

    pub(in crate::playback) fn append(
        &mut self,
        output: &mut impl Write,
        fragment: &AudioFragment,
        input: &mut impl Read,
    ) -> Result<()> {
        for sample in &fragment.samples {
            self.muxer.append_sample(&Sample {
                sample_entry: self.first_entry.take(),
                data_offset: self.position,
                track_kind: TrackKind::Audio,
                timescale: self.timescale,
                duration: sample.duration,
                keyframe: sample.keyframe,
                composition_time_offset: sample.composition_time_offset,
                data_size: sample.byte_len,
            })?;
            self.position = self
                .position
                .checked_add(u64::try_from(sample.byte_len)?)
                .ok_or(Error::Overflow)?;
        }
        let copied = io::copy(&mut input.take(fragment.byte_len), output)?;
        if copied != fragment.byte_len {
            return Err(Error::InvalidMedia {
                field: "prepared sample length",
            });
        }
        Ok(())
    }

    pub(in crate::playback) fn finish(mut self, output: &mut (impl Write + Seek)) -> Result<u64> {
        let mut byte_len = self.position;
        for (position, bytes) in self.muxer.finalize()?.offset_and_bytes_pairs() {
            let bytes = metadata_bytes(bytes)?;
            output.seek(SeekFrom::Start(position))?;
            output.write_all(&bytes)?;
            byte_len = byte_len.max(
                position
                    .checked_add(u64::try_from(bytes.len())?)
                    .ok_or(Error::Overflow)?,
            );
        }
        Ok(byte_len)
    }
}

/// Preserve sample offsets while using an mdat header understood by the tag writer.
fn metadata_bytes(bytes: &[u8]) -> Result<Vec<u8>> {
    let (header, _) = BoxHeader::decode(bytes)?;
    if header.box_type == MdatBox::TYPE
        && let BoxSize::U64(size) = header.box_size
    {
        let media_size = size
            .checked_sub(8)
            .ok_or(Error::InvalidMedia { field: "mdat size" })?;
        let padding = BoxHeader {
            box_type: FreeBox::TYPE,
            box_size: BoxSize::U32(8),
        };
        let media = BoxHeader {
            box_type: MdatBox::TYPE,
            box_size: BoxSize::U32(u32::try_from(media_size)?),
        };
        let mut output = padding.encode_to_vec()?;
        output.extend(media.encode_to_vec()?);
        return Ok(output);
    }
    Ok(bytes.to_vec())
}
