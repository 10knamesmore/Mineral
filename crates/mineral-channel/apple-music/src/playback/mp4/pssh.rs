//! Reads the Widevine initialization payload from an enhanced playlist's v0 PSSH box.

use shiguredo_mp4::{BoxHeader, BoxType, Decode};

use crate::playback::error::{Error, Result};

pub(in crate::playback) fn widevine_pssh_payload(data: &[u8]) -> Result<Vec<u8>> {
    let (header, header_size) = BoxHeader::decode(data)?;
    if header.box_type != BoxType::Normal(*b"pssh")
        || header.box_size.get() != u64::try_from(data.len())?
    {
        return Err(Error::InvalidMedia {
            field: "Widevine PSSH box",
        });
    }
    let payload = data.get(header_size..).ok_or(Error::InvalidMedia {
        field: "Widevine PSSH header",
    })?;
    let (version_and_flags, payload) =
        payload
            .split_first_chunk::<4>()
            .ok_or(Error::InvalidMedia {
                field: "Widevine PSSH version",
            })?;
    if version_and_flags != &[0; 4] {
        return Err(Error::Unsupported {
            layout: "non-v0 Widevine PSSH",
        });
    }
    let (system_id, payload) = payload
        .split_first_chunk::<16>()
        .ok_or(Error::InvalidMedia {
            field: "Widevine PSSH system ID",
        })?;
    if system_id
        != &[
            0xed, 0xef, 0x8b, 0xa9, 0x79, 0xd6, 0x4a, 0xce, 0xa3, 0xc8, 0x27, 0xdc, 0xd5, 0x1d,
            0x21, 0xed,
        ]
    {
        return Err(Error::InvalidMedia {
            field: "Widevine PSSH system ID",
        });
    }
    let (length, initialization) = payload
        .split_first_chunk::<4>()
        .ok_or(Error::InvalidMedia {
            field: "Widevine PSSH data length",
        })?;
    if usize::try_from(u32::from_be_bytes(*length))? != initialization.len() {
        return Err(Error::InvalidMedia {
            field: "Widevine PSSH data length",
        });
    }
    Ok(initialization.to_vec())
}
