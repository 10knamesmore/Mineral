//! Reads the track timing and sample layout carried by a downloaded moof box.

use shiguredo_mp4::boxes::{MdatBox, MoofBox};
use shiguredo_mp4::{BoxHeader, Decode};

use crate::playback::cenc::AudioInitialization;
use crate::playback::error::{Error, Result};

/// Sample layout from one moof, used for both progressive packets and full-file export.
pub(in crate::playback) struct AudioFragment {
    /// tfdt origin in track ticks; absent origins are resolved from preceding headers.
    pub decode_start: Option<u64>,

    /// Sum of sample durations in the initialization track's ticks.
    pub duration: u64,

    /// Total clear AAC sample bytes, excluding container metadata.
    pub byte_len: u64,

    pub(super) samples: Vec<FragmentSample>,
}

/// Sample properties retained for regular MP4 assembly, in source order.
pub(super) struct FragmentSample {
    pub(super) duration: u32,

    pub(super) byte_len: usize,

    pub(super) keyframe: bool,

    pub(super) composition_time_offset: Option<i64>,
}

impl AudioFragment {
    pub(in crate::playback) fn parse(
        moof: &MoofBox,
        initialization: &AudioInitialization,
    ) -> Result<Self> {
        let [track] = moof.traf_boxes.as_slice() else {
            return Err(Error::Unsupported {
                layout: "multiple fragment tracks",
            });
        };
        if track.tfhd_box.track_id != initialization.track_id() {
            return Err(Error::InvalidMedia {
                field: "fragment track identity",
            });
        }
        let defaults = initialization.fragment_defaults();
        let mut fragment = Self {
            decode_start: track
                .tfdt_box
                .as_ref()
                .map(|time| time.base_media_decode_time),
            duration: 0,
            byte_len: 0,
            samples: Vec::new(),
        };
        for run in &track.trun_boxes {
            for (index, sample) in run.samples.iter().enumerate() {
                let duration = sample
                    .duration
                    .or(track.tfhd_box.default_sample_duration)
                    .unwrap_or(defaults.default_sample_duration);
                let size = sample
                    .size
                    .or(track.tfhd_box.default_sample_size)
                    .unwrap_or(defaults.default_sample_size);
                if duration == 0 || size == 0 {
                    return Err(Error::InvalidMedia {
                        field: "audio sample duration or size",
                    });
                }
                let flags = (index == 0)
                    .then_some(run.first_sample_flags)
                    .flatten()
                    .or(sample.flags)
                    .or(track.tfhd_box.default_sample_flags)
                    .unwrap_or(defaults.default_sample_flags);
                fragment.samples.push(FragmentSample {
                    duration,
                    keyframe: !flags.sample_is_non_sync_sample(),
                    composition_time_offset: sample.composition_time_offset,
                    byte_len: usize::try_from(size)?,
                });
                fragment.duration = fragment
                    .duration
                    .checked_add(u64::from(duration))
                    .ok_or(Error::Overflow)?;
                fragment.byte_len = fragment
                    .byte_len
                    .checked_add(u64::from(size))
                    .ok_or(Error::Overflow)?;
            }
        }
        if fragment.samples.is_empty() {
            return Err(Error::InvalidMedia {
                field: "empty audio fragment",
            });
        }
        Ok(fragment)
    }
}

pub(in crate::playback) fn fragment_from_bytes(bytes: &[u8]) -> Result<MoofBox> {
    let mut position = 0;
    loop {
        let remaining = bytes.get(position..).ok_or(Error::InvalidMedia {
            field: "fragment box",
        })?;
        let (header, _) = BoxHeader::decode(remaining)?;
        let size = usize::try_from(header.box_size.get())?;
        if header.box_type == MoofBox::TYPE {
            return Ok(MoofBox::decode(remaining)?.0);
        }
        if size == 0 || header.box_type == MdatBox::TYPE {
            return Err(Error::InvalidMedia {
                field: "fragment header before media",
            });
        }
        position = position.checked_add(size).ok_or(Error::Overflow)?;
    }
}
