use std::borrow::Cow;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::num::{NonZeroU32, NonZeroUsize};

use reqwest::Client;
use shiguredo_mp4::boxes::{FreeBox, MdatBox, MoofBox, SampleEntry};
use shiguredo_mp4::mux::{Mp4FileMuxer, Sample};
use shiguredo_mp4::{BoxHeader, BoxSize, Decode, Encode, TrackKind};

use super::cenc::AudioInitialization;
use super::error::{Error, Result};
use super::transport::{Part, read_header_prefix, read_header_range};

pub(super) struct Fragment {
    /// tfdt origin in track ticks; absent origins are resolved from preceding headers.
    pub decode_start: Option<u64>,

    pub duration: u64,

    pub byte_len: u64,

    samples: Vec<FragmentSample>,
}

struct FragmentSample {
    duration: u32,

    byte_len: usize,

    keyframe: bool,

    composition_time_offset: Option<i64>,
}

impl Fragment {
    pub(super) fn parse(moof: &MoofBox, initialization: &AudioInitialization) -> Result<Self> {
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

/// One regular-MP4 builder serves both segment demuxing and complete file export.
pub(super) struct RegularMp4 {
    muxer: Mp4FileMuxer,

    first_entry: Option<SampleEntry>,

    position: u64,

    timescale: NonZeroU32,
}

impl RegularMp4 {
    pub(super) fn new(
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

    pub(super) fn position(&self) -> u64 {
        self.position
    }

    pub(super) fn append(
        &mut self,
        output: &mut impl Write,
        fragment: &Fragment,
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

    pub(super) fn finish(mut self, output: &mut (impl Write + Seek)) -> Result<u64> {
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

pub(super) fn fragment_from_bytes(bytes: &[u8]) -> Result<MoofBox> {
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

pub(super) async fn fragment_header(
    client: &Client,
    part: &Part,
    prefetch_bytes: NonZeroUsize,
) -> Result<MoofBox> {
    let prefix = read_header_prefix(client, part, prefetch_bytes).await?;
    let mut position = 0_usize;
    loop {
        let bytes = header_bytes(client, part, &prefix, position, BoxHeader::MAX_SIZE).await?;
        let (header, _) = BoxHeader::decode(bytes.as_ref())?;
        let size = usize::try_from(header.box_size.get())?;
        if header.box_type == MoofBox::TYPE {
            let bytes = header_bytes(client, part, &prefix, position, size).await?;
            return Ok(MoofBox::decode(bytes.as_ref())?.0);
        }
        if size == 0 || header.box_type == MdatBox::TYPE {
            return Err(Error::InvalidMedia {
                field: "fragment header before media",
            });
        }
        position = position.checked_add(size).ok_or(Error::Overflow)?;
    }
}

async fn header_bytes<'a>(
    client: &Client,
    part: &Part,
    prefix: &'a [u8],
    start: usize,
    length: usize,
) -> Result<Cow<'a, [u8]>> {
    let end = start.checked_add(length).ok_or(Error::Overflow)?;
    match prefix.get(start..end) {
        Some(bytes) => Ok(Cow::Borrowed(bytes)),
        None => Ok(Cow::Owned(
            read_header_range(client, part, start, length).await?,
        )),
    }
}
