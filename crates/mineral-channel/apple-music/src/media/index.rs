use std::borrow::Cow;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::time::Instant;

use futures_util::{StreamExt, TryStreamExt, stream};
use reqwest::Client;
use shiguredo_mp4::boxes::{FreeBox, MdatBox, MoofBox, SampleEntry, TrafBox, TrunBox, TrunSample};
use shiguredo_mp4::mux::{Mp4FileMuxer, Sample};
use shiguredo_mp4::{BoxHeader, BoxSize, Decode, Encode, SampleFlags, TrackKind};

use super::cenc::AudioInitialization;
use super::error::{Error, Result};
use super::transport::{Part, read_header_prefix, read_header_range};

pub(super) struct MetadataRange {
    pub range: Range<u64>,

    pub bytes: Vec<u8>,
}

pub(super) struct SegmentRange {
    pub range: Range<u64>,

    pub source: Part,
}

/// Sample tables define the full byte layout; audio sample data is prepared separately.
pub(super) struct AudioIndex {
    pub metadata: Vec<MetadataRange>,

    pub segments: Vec<SegmentRange>,

    pub byte_len: u64,

    pub bitrate_bps: Option<u32>,
}

pub(super) async fn read_fragment_headers(
    client: &Client,
    parts: &[Part],
    concurrency: NonZeroUsize,
    prefetch_bytes: NonZeroUsize,
) -> Result<Vec<MoofBox>> {
    let started = Instant::now();
    let mut fragments: Vec<_> = stream::iter(parts.iter().cloned().enumerate())
        .map(|(position, part)| {
            let client = client.clone();
            async move {
                let header = fragment_header(&client, &part, prefetch_bytes).await?;
                Ok::<_, Error>((position, header))
            }
        })
        .buffer_unordered(concurrency.get())
        .try_collect()
        .await?;
    mineral_log::debug!(target: "apple_music", segments = parts.len(), elapsed_ms = started.elapsed().as_millis(), "fragment headers loaded");
    fragments.sort_unstable_by_key(|(position, _)| *position);
    Ok(fragments.into_iter().map(|(_, header)| header).collect())
}

pub(super) fn build(
    parts: Vec<Part>,
    fragments: Vec<MoofBox>,
    initialization: &AudioInitialization,
) -> Result<AudioIndex> {
    let mut builder = IndexBuilder::new(initialization)?;
    for (fragment, source) in fragments.into_iter().zip(parts) {
        builder.append(&fragment, source)?;
    }
    builder.finish()
}

struct IndexBuilder<'a> {
    initialization: &'a AudioInitialization,

    muxer: Mp4FileMuxer,

    head: Vec<u8>,

    segments: Vec<SegmentRange>,

    position: u64,

    duration_ticks: u64,

    first_entry: Option<SampleEntry>,
}

impl<'a> IndexBuilder<'a> {
    fn new(initialization: &'a AudioInitialization) -> Result<Self> {
        let muxer = Mp4FileMuxer::new()?;
        let head = muxer.initial_boxes_bytes().to_vec();
        Ok(Self {
            initialization,
            position: u64::try_from(head.len())?,
            muxer,
            head,
            segments: Vec::new(),
            duration_ticks: 0,
            first_entry: Some(initialization.clear_sample_entry().clone()),
        })
    }

    fn append(&mut self, fragment: &MoofBox, source: Part) -> Result<()> {
        let [track] = fragment.traf_boxes.as_slice() else {
            return Err(Error::Unsupported {
                layout: "multiple fragment tracks",
            });
        };
        if track.tfhd_box.track_id != self.initialization.track_id() {
            return Err(Error::InvalidMedia {
                field: "fragment track identity",
            });
        }
        if track
            .tfdt_box
            .as_ref()
            .is_some_and(|time| time.base_media_decode_time != self.duration_ticks)
        {
            return Err(Error::Unsupported {
                layout: "non-contiguous decode timeline",
            });
        }
        let start = self.position;
        for run in &track.trun_boxes {
            for (index, sample) in run.samples.iter().enumerate() {
                self.append_sample(track, run, sample, index)?;
            }
        }
        if self.position == start {
            return Err(Error::InvalidMedia {
                field: "empty audio fragment",
            });
        }
        self.segments.push(SegmentRange {
            range: start..self.position,
            source,
        });
        Ok(())
    }

    fn append_sample(
        &mut self,
        track: &TrafBox,
        run: &TrunBox,
        sample: &TrunSample,
        index: usize,
    ) -> Result<()> {
        let defaults = self.initialization.fragment_defaults();
        let duration = sample
            .duration
            .or(track.tfhd_box.default_sample_duration)
            .unwrap_or(defaults.default_sample_duration);
        let size = sample
            .size
            .or(track.tfhd_box.default_sample_size)
            .unwrap_or(defaults.default_sample_size);
        let flags = sample_flags(track, run, sample, index, defaults.default_sample_flags);
        self.muxer.append_sample(&Sample {
            track_kind: TrackKind::Audio,
            sample_entry: self.first_entry.take(),
            timescale: self.initialization.timescale(),
            duration,
            keyframe: !flags.sample_is_non_sync_sample(),
            composition_time_offset: sample.composition_time_offset,
            data_offset: self.position,
            data_size: usize::try_from(size)?,
        })?;
        self.position = self
            .position
            .checked_add(u64::from(size))
            .ok_or(Error::Overflow)?;
        self.duration_ticks = self
            .duration_ticks
            .checked_add(u64::from(duration))
            .ok_or(Error::Overflow)?;
        Ok(())
    }

    fn finish(mut self) -> Result<AudioIndex> {
        let mut byte_len = self.position;
        let mut metadata = Vec::new();
        for (position, bytes) in self.muxer.finalize()?.offset_and_bytes_pairs() {
            let bytes = metadata_bytes(bytes)?;
            if position < u64::try_from(self.head.len())? {
                let start = usize::try_from(position)?;
                let end = start.checked_add(bytes.len()).ok_or(Error::Overflow)?;
                self.head
                    .get_mut(start..end)
                    .ok_or(Error::InvalidMedia {
                        field: "MP4 header range",
                    })?
                    .copy_from_slice(&bytes);
            } else {
                let end = position
                    .checked_add(u64::try_from(bytes.len())?)
                    .ok_or(Error::Overflow)?;
                byte_len = byte_len.max(end);
                metadata.push(MetadataRange {
                    range: position..end,
                    bytes,
                });
            }
        }
        metadata.push(MetadataRange {
            range: 0..u64::try_from(self.head.len())?,
            bytes: self.head,
        });
        let bitrate_bps = u128::from(byte_len)
            .checked_mul(8)
            .and_then(|bits| bits.checked_mul(u128::from(self.initialization.timescale().get())))
            .and_then(|scaled| scaled.checked_div(u128::from(self.duration_ticks)))
            .and_then(|rate| u32::try_from(rate).ok());
        Ok(AudioIndex {
            metadata,
            segments: self.segments,
            byte_len,
            bitrate_bps,
        })
    }
}

fn sample_flags(
    track: &TrafBox,
    run: &TrunBox,
    sample: &TrunSample,
    index: usize,
    default: SampleFlags,
) -> SampleFlags {
    if index == 0
        && let Some(flags) = run.first_sample_flags
    {
        return flags;
    }
    sample
        .flags
        .or(track.tfhd_box.default_sample_flags)
        .unwrap_or(default)
}

/// Keep sample positions unchanged while using the mdat header understood by the tag writer.
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

async fn fragment_header(
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
            mineral_log::debug!(target: "apple_music", offset = position, bytes = size, prefix_bytes = prefix.len(), "fragment index header");
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

/// Use the fetched prefix when it covers the box; otherwise request that box's exact range.
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
