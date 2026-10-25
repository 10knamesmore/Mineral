use shiguredo_mp4::{BoxHeader, BoxSize, BoxType};

use super::error::{Error, Result, Unsupported};

#[derive(Clone, Copy)]
pub(super) struct BoxSlice<'a> {
    pub kind: BoxType,

    pub start: usize,

    pub payload_start: usize,

    pub bytes: &'a [u8],

    pub payload: &'a [u8],
}

pub(super) fn boxes(bytes: &[u8], origin: usize) -> Result<Vec<BoxSlice<'_>>> {
    let mut result = Vec::new();
    let mut offset = 0;
    let mut unparsed = bytes;
    while !unparsed.is_empty() {
        let (header, payload) = BoxHeader::decode_header_and_payload(unparsed)?;
        if header.box_size == BoxSize::VARIABLE_SIZE && header.box_type != BoxType::Normal(*b"mdat")
        {
            return Err(invalid(*b"box ", "size=0 is supported only for final mdat"));
        }
        let external_size = header.external_size();
        let length = add(external_size, payload.len())?;
        let start = add(origin, offset)?;
        let (box_bytes, rest) = unparsed
            .split_at_checked(length)
            .ok_or_else(|| invalid(*b"box ", "length exceeds the remaining bytes"))?;
        result.push(BoxSlice {
            kind: header.box_type,
            start,
            payload_start: add(start, external_size)?,
            bytes: box_bytes,
            payload,
        });
        unparsed = rest;
        offset = add(offset, length)?;
    }
    Ok(result)
}

pub(super) fn optional<'a>(boxes: &[BoxSlice<'a>], kind: [u8; 4]) -> Result<Option<BoxSlice<'a>>> {
    let mut matches = boxes.iter().filter(|b| b.kind == BoxType::Normal(kind));
    let found = matches.next().copied();
    if matches.next().is_some() {
        return Err(Error::DuplicateBox(kind));
    }
    Ok(found)
}

pub(super) fn required<'a>(boxes: &[BoxSlice<'a>], kind: [u8; 4]) -> Result<BoxSlice<'a>> {
    optional(boxes, kind)?.ok_or(Error::MissingBox(kind))
}

pub(super) fn invalid(box_type: [u8; 4], field: &'static str) -> Error {
    Error::InvalidField { box_type, field }
}

pub(super) fn add(left: usize, right: usize) -> Result<usize> {
    left.checked_add(right).ok_or(Error::ArithmeticOverflow)
}

pub(super) fn to_usize(value: u64) -> Result<usize> {
    usize::try_from(value).map_err(|_too_large| Error::ArithmeticOverflow)
}

pub(super) fn to_u64(value: usize) -> Result<u64> {
    u64::try_from(value).map_err(|_too_large| Error::ArithmeticOverflow)
}

pub(super) fn reject_encryption_extension(kind: BoxType, payload: &[u8]) -> Result<()> {
    match kind {
        BoxType::Uuid(_) => Err(Error::Unsupported(Unsupported::UuidEncryptionBox)),
        BoxType::Normal(kind) if kind == *b"sgpd" || kind == *b"sbgp" => {
            let mut reader = Reader::new(payload, kind);
            reader.take(4)?;
            if reader.take(4)? == b"seig" {
                return Err(Error::Unsupported(Unsupported::SampleEncryptionGroups));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub(super) struct Reader<'a> {
    remaining: &'a [u8],

    pub kind: [u8; 4],
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8], kind: [u8; 4]) -> Self {
        Self {
            remaining: bytes,
            kind,
        }
    }

    pub fn remaining(&self) -> usize {
        self.remaining.len()
    }

    pub fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        if length > self.remaining.len() {
            return Err(Error::Truncated {
                box_type: self.kind,
                needed: length,
                remaining: self.remaining.len(),
            });
        }
        let (value, rest) = self.remaining.split_at(length);
        self.remaining = rest;
        Ok(value)
    }

    fn take_array<const LENGTH: usize>(&mut self) -> Result<[u8; LENGTH]> {
        let mut value = [0; LENGTH];
        value.copy_from_slice(self.take(LENGTH)?);
        Ok(value)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(u8::from_be_bytes(self.take_array()?))
    }

    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.take_array()?))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take_array()?))
    }

    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.take_array()?))
    }

    pub fn fourcc(&mut self) -> Result<[u8; 4]> {
        self.take_array()
    }

    /// ISO full-box flags: the 24 bits that follow the version byte.
    fn flags(&mut self) -> Result<u32> {
        let [high, middle, low] = self.take_array()?;
        Ok(u32::from_be_bytes([0, high, middle, low]))
    }

    pub fn full_header(&mut self, max_version: u8, allowed_flags: u32) -> Result<(u8, u32)> {
        let version = self.u8()?;
        let flags = self.flags()?;
        if version > max_version || flags & !allowed_flags != 0 {
            return Err(Error::BoxVersionOrFlags {
                box_type: self.kind,
                version,
                flags,
            });
        }
        Ok((version, flags))
    }

    pub fn finish(self) -> Result<()> {
        if self.remaining.is_empty() {
            Ok(())
        } else {
            Err(invalid(self.kind, "trailing bytes"))
        }
    }
}
