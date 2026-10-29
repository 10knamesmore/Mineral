//! Reads per-sample IVs and subsample encryption ranges from fragment metadata.

use std::ops::Range;

use super::binary::{BoxSlice, Reader, add, invalid, optional, to_usize};
use super::error::{Error, Result, Unsupported};
use super::initialization::InitializationVector;

pub(super) struct SampleEncryption {
    pub iv: [u8; 16],

    pub subsamples: Vec<Subsample>,

    pub storage: Range<usize>,
}

pub(super) struct Subsample {
    pub clear_bytes: usize,

    pub encrypted_bytes: usize,
}

#[derive(Debug, PartialEq, Eq)]
struct AuxiliaryType {
    kind: [u8; 4],

    parameter: u32,
}

pub(super) fn sample_encryption(
    fragment: &[u8],
    children: &[BoxSlice<'_>],
    initialization_vector: InitializationVector,
    run_counts: &[usize],
    moof_start: usize,
) -> Result<Vec<SampleEncryption>> {
    let sample_count = run_counts
        .iter()
        .try_fold(0, |sum, count| add(sum, *count))?;
    let senc = optional(children, *b"senc")?;
    let saiz = optional(children, *b"saiz")?;
    let saio = optional(children, *b"saio")?;
    let inline = senc
        .map(|value| parse_senc(value, initialization_vector, sample_count))
        .transpose()?;
    match (saiz, saio) {
        (None, None) => inline.ok_or(Error::MissingBox(*b"senc")),
        (Some(sizes_box), Some(offsets_box)) => {
            let (sizes_type, sizes) = parse_saiz(sizes_box.payload, sample_count)?;
            let (offsets_type, offsets) = parse_saio(offsets_box.payload, run_counts.len())?;
            if sizes_type != offsets_type {
                return Err(invalid(*b"saio", "aux_info_type differs from saiz"));
            }
            let ranges = auxiliary_ranges(&sizes, &offsets, run_counts, moof_start)?;
            if let Some(records) = inline {
                for (sample, (record, range)) in records.iter().zip(&ranges).enumerate() {
                    if record.storage != *range {
                        return Err(Error::AuxiliaryMismatch { sample });
                    }
                }
                return Ok(records);
            }
            let mut records = Vec::with_capacity(sample_count);
            for range in ranges {
                let bytes = fragment.get(range.clone()).ok_or_else(|| {
                    invalid(*b"saio", "auxiliary range outside supplied fragment")
                })?;
                let mut reader = Reader::new(bytes, *b"saiz");
                let has_subsamples = bytes.len() > initialization_vector.per_sample_size();
                let record = parse_record(
                    &mut reader,
                    initialization_vector,
                    has_subsamples,
                    range.start,
                )?;
                reader.finish()?;
                records.push(record);
            }
            Ok(records)
        }
        _ => Err(Error::Unsupported(Unsupported::AuxiliaryLayout)),
    }
}

fn parse_senc(
    senc: BoxSlice<'_>,
    initialization_vector: InitializationVector,
    sample_count: usize,
) -> Result<Vec<SampleEncryption>> {
    let mut reader = Reader::new(senc.payload, *b"senc");
    let (_, flags) = reader.full_header(0, 3)?;
    if flags & 1 != 0 {
        return Err(Error::Unsupported(Unsupported::SampleEncryptionOverride));
    }
    let declared_count = to_usize(u64::from(reader.u32()?))?;
    check_count(*b"senc", sample_count, declared_count)?;
    let mut records = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        let start = add(senc.payload_start, senc.payload.len() - reader.remaining())?;
        records.push(parse_record(
            &mut reader,
            initialization_vector,
            flags & 2 != 0,
            start,
        )?);
    }
    reader.finish()?;
    Ok(records)
}

fn parse_record(
    reader: &mut Reader<'_>,
    initialization_vector: InitializationVector,
    has_subsamples: bool,
    start: usize,
) -> Result<SampleEncryption> {
    let remaining = reader.remaining();
    let iv = match initialization_vector {
        InitializationVector::Constant(iv) => iv,
        InitializationVector::PerSample(size) => {
            // Eight-byte IVs occupy the high half of the counter block.
            let mut iv = [0; 16];
            for (target, byte) in iv.iter_mut().zip(reader.take(usize::from(size))?) {
                *target = *byte;
            }
            iv
        }
    };
    let mut subsamples = Vec::new();
    if has_subsamples {
        let count = reader.u16()?;
        for _ in 0..count {
            subsamples.push(Subsample {
                clear_bytes: usize::from(reader.u16()?),
                encrypted_bytes: to_usize(u64::from(reader.u32()?))?,
            });
        }
    }
    Ok(SampleEncryption {
        iv,
        subsamples,
        storage: start..add(start, remaining - reader.remaining())?,
    })
}

fn auxiliary_type(reader: &mut Reader<'_>, flags: u32) -> Result<Option<AuxiliaryType>> {
    if flags & 1 == 0 {
        return Ok(None);
    }
    let kind = reader.fourcc()?;
    let parameter = reader.u32()?;
    if ![*b"cenc", *b"cbcs"].contains(&kind) || parameter != 0 {
        return Err(Error::Unsupported(Unsupported::AuxiliaryType));
    }
    Ok(Some(AuxiliaryType { kind, parameter }))
}

fn parse_saiz(payload: &[u8], sample_count: usize) -> Result<(Option<AuxiliaryType>, Vec<u8>)> {
    let mut reader = Reader::new(payload, *b"saiz");
    let (_, flags) = reader.full_header(0, 1)?;
    let info_type = auxiliary_type(&mut reader, flags)?;
    let default_size = reader.u8()?;
    let declared_count = to_usize(u64::from(reader.u32()?))?;
    check_count(*b"saiz", sample_count, declared_count)?;
    let sizes = if default_size == 0 {
        reader.take(sample_count)?.to_vec()
    } else {
        vec![default_size; sample_count]
    };
    reader.finish()?;
    Ok((info_type, sizes))
}

fn parse_saio(payload: &[u8], run_count: usize) -> Result<(Option<AuxiliaryType>, Vec<u64>)> {
    let mut reader = Reader::new(payload, *b"saio");
    let (version, flags) = reader.full_header(1, 1)?;
    let info_type = auxiliary_type(&mut reader, flags)?;
    let count = to_usize(u64::from(reader.u32()?))?;
    if count != 1 && count != run_count {
        return Err(Error::Unsupported(Unsupported::AuxiliaryLayout));
    }
    let mut offsets = Vec::new();
    for _ in 0..count {
        offsets.push(if version == 0 {
            u64::from(reader.u32()?)
        } else {
            reader.u64()?
        });
    }
    reader.finish()?;
    Ok((info_type, offsets))
}

/// `offsets` carries either one entry for the whole fragment or one per `trun`, as
/// restricted by `parse_saio`; `sizes` carries one entry per sample.
fn auxiliary_ranges(
    sizes: &[u8],
    offsets: &[u64],
    run_counts: &[usize],
    moof_start: usize,
) -> Result<Vec<Range<usize>>> {
    match offsets {
        [offset] => contiguous_ranges(sizes, *offset, moof_start),
        per_run => run_ranges(sizes, per_run, run_counts, moof_start),
    }
}

/// Records addressed by one `saio` offset run consecutively across all `trun` samples.
fn contiguous_ranges(sizes: &[u8], offset: u64, moof_start: usize) -> Result<Vec<Range<usize>>> {
    let mut ranges = Vec::with_capacity(sizes.len());
    let mut cursor = add(moof_start, to_usize(offset)?)?;
    for size in sizes {
        let end = add(cursor, usize::from(*size))?;
        ranges.push(cursor..end);
        cursor = end;
    }
    Ok(ranges)
}

/// With per-run offsets, every `trun` states where its own records begin.
fn run_ranges(
    sizes: &[u8],
    offsets: &[u64],
    run_counts: &[usize],
    moof_start: usize,
) -> Result<Vec<Range<usize>>> {
    let mut ranges = Vec::with_capacity(sizes.len());
    let mut remaining_sizes = sizes.iter();
    for (offset, run_count) in offsets.iter().zip(run_counts) {
        let mut cursor = add(moof_start, to_usize(*offset)?)?;
        for size in remaining_sizes.by_ref().take(*run_count) {
            let end = add(cursor, usize::from(*size))?;
            ranges.push(cursor..end);
            cursor = end;
        }
    }
    Ok(ranges)
}

fn check_count(kind: [u8; 4], expected: usize, actual: usize) -> Result<()> {
    if actual != expected {
        Err(Error::SampleCount {
            box_type: kind,
            expected,
            actual,
        })
    } else {
        Ok(())
    }
}
