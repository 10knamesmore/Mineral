use std::ops::Range;

use aes::Aes128;
use ctr::Ctr128BE;
use ctr::cipher::{KeyIvInit, StreamCipher};
use shiguredo_mp4::boxes::{MoofBox, TrafBox};
use shiguredo_mp4::{BoxType, Decode};

use super::auxiliary::{SampleEncryption, sample_encryption};
use super::binary::{
    BoxSlice, Reader, add, boxes, invalid, optional, reject_encryption_extension, required, to_u64,
    to_usize,
};
use super::error::{Error, Result, Unsupported};
use super::initialization::AudioInitialization;

#[derive(Debug, Clone)]
struct SampleRange {
    /// Byte range in the concatenated clear AAC access units.
    clear_range: Range<usize>,

    /// Byte range in the encrypted fragment slice passed to the decrypt method.
    encrypted_range: Range<usize>,
}

impl AudioInitialization {
    /// Decrypt a complete media segment containing exactly one moof/traf.
    /// `key_id` is the raw 16-byte CENC KID, in on-wire order, not a GUID byte swap.
    /// AES-CTR is unauthenticated: a matching KID cannot establish that a key is correct.
    ///
    /// `fragment_file_offset` is the resource byte offset of the supplied slice.
    /// It is used only to resolve an explicit absolute `tfhd.base_data_offset`.
    /// Auxiliary offsets must be moof-relative; explicit non-moof auxiliary bases
    /// are rejected rather than guessed.
    pub fn decrypt_fragment_at(
        &self,
        encrypted_fragment: &[u8],
        fragment_file_offset: u64,
        content_key: &[u8; 16],
        key_id: &[u8; 16],
    ) -> Result<Vec<u8>> {
        if key_id != self.key_id() {
            return Err(Error::KeyIdMismatch);
        }
        let top = boxes(encrypted_fragment, 0)?;
        if top
            .iter()
            .filter(|value| value.kind == MoofBox::TYPE)
            .count()
            != 1
        {
            return Err(Error::Unsupported(Unsupported::MultipleMovieFragments));
        }
        let moof_bytes = required(&top, *b"moof")?;
        let moof_children = boxes(moof_bytes.payload, moof_bytes.payload_start)?;
        let traf_bytes = required(&moof_children, *b"traf")?;
        let children = boxes(traf_bytes.payload, traf_bytes.payload_start)?;
        validate_fragment_headers(&moof_children, &children)?;
        let (moof, _) = MoofBox::decode(moof_bytes.bytes)?;
        let [track] = moof.traf_boxes.as_slice() else {
            return Err(Error::Unsupported(Unsupported::MultipleTrackFragments));
        };
        let header = &track.tfhd_box;
        if header.track_id != self.track_id() {
            return Err(invalid(*b"tfhd", "track_id differs from initialization"));
        }
        if header
            .sample_description_index
            .unwrap_or(self.fragment_defaults().default_sample_description_index)
            != 1
        {
            return Err(Error::Unsupported(Unsupported::SampleDescriptionChange));
        }
        if header.duration_is_empty || track.trun_boxes.iter().all(|run| run.samples.is_empty()) {
            return Err(Error::Unsupported(Unsupported::EmptyFragment));
        }
        let moof_absolute = fragment_file_offset
            .checked_add(to_u64(moof_bytes.start)?)
            .ok_or(Error::ArithmeticOverflow)?;
        if optional(&children, *b"saio")?.is_some()
            && header
                .base_data_offset
                .is_some_and(|base| base != moof_absolute)
        {
            return Err(Error::Unsupported(Unsupported::AuxiliaryLayout));
        }
        // With one traf, the implicit base is moof even without default-base-is-moof.
        let data_base = header.base_data_offset.unwrap_or(moof_absolute);
        let samples = resolve_samples(self, track, data_base, fragment_file_offset, &top)?;
        let run_counts: Vec<_> = track
            .trun_boxes
            .iter()
            .map(|run| run.samples.len())
            .collect();
        let records = sample_encryption(
            encrypted_fragment,
            &children,
            self.encryption.iv_size,
            &run_counts,
            moof_bytes.start,
        )?;
        validate_auxiliary_storage(&records, &samples, &children, &top)?;
        let mut bytes = Vec::new();
        for (sample_index, (sample, record)) in samples.iter().zip(&records).enumerate() {
            let encrypted = encrypted_fragment
                .get(sample.encrypted_range.clone())
                .ok_or_else(|| invalid(*b"trun", "sample range outside the fragment"))?;
            bytes.extend_from_slice(encrypted);
            let clear = bytes
                .get_mut(sample.clear_range.clone())
                .ok_or_else(|| invalid(*b"trun", "sample clear range outside the output"))?;
            decrypt_sample(clear, record, content_key, sample_index)?;
        }
        Ok(bytes)
    }
}

fn resolve_samples(
    initialization: &AudioInitialization,
    track: &TrafBox,
    data_base: u64,
    fragment_file_offset: u64,
    top: &[BoxSlice<'_>],
) -> Result<Vec<SampleRange>> {
    let header = &track.tfhd_box;
    let defaults = initialization.fragment_defaults();
    let mut samples = Vec::new();
    let mut previous_run_end = None;
    let mut clear_offset = 0;
    for run in &track.trun_boxes {
        let mut cursor = match run.data_offset {
            Some(offset) => data_base.checked_add_signed(i64::from(offset)),
            None => Some(previous_run_end.unwrap_or(data_base)),
        }
        .ok_or(Error::ArithmeticOverflow)?;
        for sample in &run.samples {
            let sample_size = sample
                .size
                .or(header.default_sample_size)
                .unwrap_or(defaults.default_sample_size);
            let duration = sample
                .duration
                .or(header.default_sample_duration)
                .unwrap_or(defaults.default_sample_duration);
            if sample_size == 0 || duration == 0 {
                return Err(invalid(*b"trun", "AAC sample size or duration is zero"));
            }
            let size = to_usize(u64::from(sample_size))?;
            let offset =
                cursor
                    .checked_sub(fragment_file_offset)
                    .ok_or(Error::SampleOutsideMedia {
                        sample: samples.len(),
                        offset: cursor,
                        size,
                    })?;
            let start = to_usize(offset)?;
            let end = add(start, size)?;
            if !inside_mdat(start..end, top) {
                return Err(Error::SampleOutsideMedia {
                    sample: samples.len(),
                    offset: cursor,
                    size,
                });
            }
            let clear_end = add(clear_offset, size)?;
            samples.push(SampleRange {
                clear_range: clear_offset..clear_end,
                encrypted_range: start..end,
            });
            clear_offset = clear_end;
            cursor = cursor
                .checked_add(u64::from(sample_size))
                .ok_or(Error::ArithmeticOverflow)?;
        }
        previous_run_end = Some(cursor);
    }
    Ok(samples)
}

fn validate_fragment_headers(moof: &[BoxSlice<'_>], traf: &[BoxSlice<'_>]) -> Result<()> {
    required(moof, *b"mfhd")?;
    required(traf, *b"tfhd")?;
    optional(traf, *b"tfdt")?;
    for child in moof.iter().chain(traf) {
        reject_encryption_extension(child.kind, child.payload)?;
        let (max_version, flags) = match child.kind {
            BoxType::Normal(kind) if kind == *b"mfhd" => (0, 0),
            BoxType::Normal(kind) if kind == *b"tfhd" => (0, 0x03003b),
            BoxType::Normal(kind) if kind == *b"tfdt" => (1, 0),
            BoxType::Normal(kind) if kind == *b"trun" => (1, 0x000f05),
            _ => continue,
        };
        if let BoxType::Normal(kind) = child.kind {
            let mut reader = Reader::new(child.payload, kind);
            let (_, actual_flags) = reader.full_header(max_version, flags)?;
            if kind == *b"trun" && actual_flags & 0x000404 == 0x000404 {
                return Err(invalid(
                    kind,
                    "first_sample_flags and sample_flags both present",
                ));
            }
        }
    }
    Ok(())
}

fn inside_mdat(range: Range<usize>, top: &[BoxSlice<'_>]) -> bool {
    top.iter().any(|value| {
        value.kind == BoxType::Normal(*b"mdat")
            && range.start >= value.payload_start
            && range.end <= value.payload_start + value.payload.len()
    })
}

fn validate_auxiliary_storage(
    records: &[SampleEncryption],
    samples: &[SampleRange],
    children: &[BoxSlice<'_>],
    top: &[BoxSlice<'_>],
) -> Result<()> {
    if optional(children, *b"senc")?.is_some() {
        return Ok(());
    }
    for record in records {
        if !inside_mdat(record.storage.clone(), top) {
            return Err(invalid(*b"saio", "external auxiliary data is not in mdat"));
        }
        if samples.iter().any(|sample| {
            record.storage.start < sample.encrypted_range.end
                && sample.encrypted_range.start < record.storage.end
        }) {
            return Err(invalid(*b"saio", "auxiliary data overlaps audio"));
        }
    }
    Ok(())
}

fn decrypt_sample(
    sample: &mut [u8],
    encryption: &SampleEncryption,
    key: &[u8; 16],
    sample_index: usize,
) -> Result<()> {
    let mut cipher = Ctr128BE::<Aes128>::new(key.into(), (&encryption.iv).into());
    if encryption.subsamples.is_empty() {
        cipher
            .try_apply_keystream(sample)
            .map_err(|_counter_exhausted| Error::CounterExhausted)?;
        return Ok(());
    }
    let sample_length = sample.len();
    let mut cursor = 0;
    for subsample in &encryption.subsamples {
        let start = add(cursor, subsample.clear_bytes)?;
        let end = add(start, subsample.encrypted_bytes)?;
        let protected = sample.get_mut(start..end).ok_or(Error::SubsampleCoverage {
            sample: sample_index,
            covered: end,
            size: sample_length,
        })?;
        // Clear bytes do not consume keystream. Keep CTR state across protected ranges.
        cipher
            .try_apply_keystream(protected)
            .map_err(|_counter_exhausted| Error::CounterExhausted)?;
        cursor = end;
    }
    if cursor != sample_length {
        return Err(Error::SubsampleCoverage {
            sample: sample_index,
            covered: cursor,
            size: sample_length,
        });
    }
    Ok(())
}
