//! Retains the audio track's codec, timing defaults and encryption parameters.

use std::num::NonZeroU32;

use shiguredo_mp4::boxes::{
    AudioSampleEntryFields, EsdsBox, MoovBox, Mp4aBox, SampleEntry, TrexBox, UnknownBox,
};
use shiguredo_mp4::{BoxType, Decode};

use super::binary::{Reader, boxes, invalid, reject_encryption_extension, required};
use super::error::{Error, Result, Unsupported};

/// Parsed single-track encrypted AAC initialization; contains no content key.
/// Retained for the open track to decrypt fragments and remux their clear samples
/// with the same codec description, timescale and fragment defaults.
#[derive(Debug, Clone)]
pub(crate) struct AudioInitialization {
    track_id: u32,

    /// Track ticks per second.
    timescale: NonZeroU32,

    clear_sample_entry: SampleEntry,

    fragment_defaults: TrexBox,

    encryption_descriptions: Vec<SampleDescriptionEncryption>,

    content_key_id: Option<[u8; 16]>,
}

/// Apple uses this public key for the prefetch description, not the licensed audio.
pub(super) const PREFETCH_KEY: [u8; 16] = [
    0x32, 0xb8, 0xad, 0xe1, 0x76, 0x9e, 0x26, 0xb1, 0xff, 0xb8, 0x98, 0x63, 0x52, 0x79, 0x3f, 0xc6,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EncryptionScheme {
    Cenc,

    Cbcs,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum InitializationVector {
    PerSample(u8),

    Constant([u8; 16]),
}

impl InitializationVector {
    pub fn per_sample_size(self) -> usize {
        match self {
            Self::PerSample(size) => usize::from(size),
            Self::Constant(_) => 0,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct SampleDescriptionEncryption {
    pub scheme: EncryptionScheme,

    pub initialization_vector: InitializationVector,

    pub key_id: [u8; 16],

    /// Enhanced CBCS reserves the first description for the public prefetch key.
    pub uses_prefetch_key: bool,
}

impl AudioInitialization {
    pub fn parse(encrypted_initialization: &[u8]) -> Result<Self> {
        let top = boxes(encrypted_initialization, 0)?;
        let moov_bytes = required(&top, *b"moov")?;
        let (moov, _) = MoovBox::decode(moov_bytes.bytes)?;
        let [track] = moov.trak_boxes.as_slice() else {
            return Err(Error::Unsupported(Unsupported::MultipleTracks));
        };
        if track.mdia_box.hdlr_box.handler_type != *b"soun" {
            return Err(Error::Unsupported(Unsupported::NonAudioTrack));
        }
        let data_reference = &track.mdia_box.minf_box.dinf_box.dref_box;
        if !data_reference.unknown_boxes.is_empty()
            || !data_reference
                .url_box
                .as_ref()
                .is_some_and(|url| url.location.is_none())
        {
            return Err(Error::Unsupported(Unsupported::ExternalDataReference));
        }
        let table = &track.mdia_box.minf_box.stbl_box;
        for extension in &table.unknown_boxes {
            reject_encryption_extension(extension.box_type, &extension.payload)?;
        }
        let mut clear_entry: Option<Mp4aBox> = None;
        let mut encryption_descriptions = Vec::new();
        let mut content_key_id = None;
        for (index, entry) in table.stsd_box.entries.iter().enumerate() {
            let SampleEntry::Unknown(entry) = entry else {
                return Err(Error::Unsupported(Unsupported::NonEncryptedAudioEntry));
            };
            if entry.box_type != BoxType::Normal(*b"enca") {
                return Err(Error::Unsupported(Unsupported::NonEncryptedAudioEntry));
            }
            let (clear, protection) = parse_audio_entry(entry, index == 0)?;
            if let Some(first) = &clear_entry {
                if first.audio != clear.audio
                    || first.esds_box.es.dec_config_descr.dec_specific_info
                        != clear.esds_box.es.dec_config_descr.dec_specific_info
                {
                    return Err(Error::Unsupported(Unsupported::AudioConfigurationChange));
                }
            } else {
                clear_entry = Some(clear);
            }
            if !protection.uses_prefetch_key {
                if content_key_id.is_some_and(|key_id| key_id != protection.key_id) {
                    return Err(Error::Unsupported(Unsupported::MultipleContentKeys));
                }
                content_key_id = Some(protection.key_id);
            }
            encryption_descriptions.push(protection);
        }
        let clear_sample_entry = SampleEntry::Mp4a(
            clear_entry.ok_or_else(|| invalid(*b"stsd", "no audio sample descriptions"))?,
        );
        let content_key_id =
            content_key_id.ok_or(Error::Unsupported(Unsupported::MissingContentDescription))?;
        let content_key_id = encryption_descriptions
            .iter()
            .any(|description| description.scheme == EncryptionScheme::Cenc)
            .then_some(content_key_id);
        let track_id = track.tkhd_box.track_id;
        let mvex = moov
            .mvex_box
            .as_ref()
            .ok_or(Error::Unsupported(Unsupported::NonFragmentedInitialization))?;
        let mut defaults = mvex
            .trex_boxes
            .iter()
            .filter(|value| value.track_id == track_id);
        let fragment_defaults = defaults.next().ok_or(Error::MissingBox(*b"trex"))?.clone();
        if defaults.next().is_some() {
            return Err(Error::DuplicateBox(*b"trex"));
        }
        description(
            &encryption_descriptions,
            fragment_defaults.default_sample_description_index,
        )?;
        Ok(Self {
            track_id,
            timescale: track.mdia_box.mdhd_box.timescale,
            clear_sample_entry,
            fragment_defaults,
            encryption_descriptions,
            content_key_id,
        })
    }

    pub fn track_id(&self) -> u32 {
        self.track_id
    }

    pub fn timescale(&self) -> NonZeroU32 {
        self.timescale
    }

    /// The `mp4a` entry retains `esds` and non-protection children, but removes `sinf`.
    pub fn clear_sample_entry(&self) -> &SampleEntry {
        &self.clear_sample_entry
    }

    /// Defaults for indexing separately fetched `MoofBox` headers before downloading media.
    pub fn fragment_defaults(&self) -> &TrexBox {
        &self.fragment_defaults
    }

    /// CENC supplies a Widevine KID in tenc. Apple's CBCS media identifies its
    /// Widevine key through the HLS PSSH instead, so no container KID is exposed.
    pub fn key_id(&self) -> Option<&[u8; 16]> {
        self.content_key_id.as_ref()
    }

    pub(super) fn encryption(
        &self,
        description_index: u32,
    ) -> Result<&SampleDescriptionEncryption> {
        description(&self.encryption_descriptions, description_index)
    }
}

fn description(
    descriptions: &[SampleDescriptionEncryption],
    index: u32,
) -> Result<&SampleDescriptionEncryption> {
    let offset = index
        .checked_sub(1)
        .and_then(|value| usize::try_from(value).ok());
    offset
        .and_then(|value| descriptions.get(value))
        .ok_or(Error::SampleDescriptionIndex {
            index,
            count: descriptions.len(),
        })
}

fn parse_audio_entry(
    entry: &UnknownBox,
    first_description: bool,
) -> Result<(Mp4aBox, SampleDescriptionEncryption)> {
    let mut fields = Reader::new(&entry.payload, *b"enca");
    fields.take(8)?;
    if fields.u16()? != 0 {
        return Err(Error::Unsupported(Unsupported::QuickTimeAudioEntry));
    }
    let (audio, consumed) = AudioSampleEntryFields::decode(&entry.payload)?;
    if audio.data_reference_index.get() != 1 {
        return Err(Error::Unsupported(Unsupported::ExternalDataReference));
    }
    let children_bytes = entry
        .payload
        .get(consumed..)
        .ok_or_else(|| invalid(*b"enca", "audio fields exceed the entry payload"))?;
    let children = boxes(children_bytes, consumed)?;
    let sinf = required(&children, *b"sinf")?;
    let protection = boxes(sinf.payload, sinf.payload_start)?;
    for child in &protection {
        match child.kind {
            BoxType::Normal(kind) if [*b"frma", *b"schm", *b"schi"].contains(&kind) => {}
            BoxType::Normal(kind) => {
                return Err(Error::Unsupported(Unsupported::ProtectionBox(kind)));
            }
            BoxType::Uuid(_) => {
                return Err(Error::Unsupported(Unsupported::UuidEncryptionBox));
            }
        }
    }
    let frma = required(&protection, *b"frma")?;
    if frma.payload != b"mp4a" {
        return Err(Error::Unsupported(Unsupported::NonAacEntry));
    }
    let schm = required(&protection, *b"schm")?;
    let mut scheme = Reader::new(schm.payload, *b"schm");
    let (_, flags) = scheme.full_header(0, 1)?;
    let scheme_type = scheme.fourcc()?;
    let encryption_scheme = match &scheme_type {
        b"cenc" => EncryptionScheme::Cenc,
        b"cbcs" => EncryptionScheme::Cbcs,
        _ => return Err(Error::Scheme(scheme_type)),
    };
    if scheme.u32()? != 0x0001_0000 {
        return Err(invalid(*b"schm", "scheme_version"));
    }
    if flags & 1 != 0 {
        let uri = scheme.take(scheme.remaining())?;
        if uri.last() != Some(&0) {
            return Err(invalid(*b"schm", "scheme_uri"));
        }
    }
    scheme.finish()?;
    let schi = required(&protection, *b"schi")?;
    let scheme_information = boxes(schi.payload, schi.payload_start)?;
    let tenc = required(&scheme_information, *b"tenc")?;
    for child in &scheme_information {
        if child.kind != BoxType::Normal(*b"tenc") {
            return Err(Error::Unsupported(Unsupported::AuxiliaryLayout));
        }
    }
    let (initialization_vector, key_id) = parse_tenc(tenc.payload, encryption_scheme)?;
    let encryption = SampleDescriptionEncryption {
        scheme: encryption_scheme,
        initialization_vector,
        key_id,
        uses_prefetch_key: encryption_scheme == EncryptionScheme::Cbcs && first_description,
    };
    let esds = required(&children, *b"esds")?;
    Reader::new(esds.payload, *b"esds").full_header(0, 0)?;
    let (esds_box, _) = EsdsBox::decode(esds.bytes)?;
    let decoder = &esds_box.es.dec_config_descr;
    if decoder.object_type_indication != 0x40 || decoder.stream_type.get() != 5 {
        return Err(Error::Unsupported(Unsupported::NonAacEntry));
    }
    if !decoder
        .dec_specific_info
        .as_ref()
        .is_some_and(|info| !info.payload.is_empty())
    {
        return Err(invalid(*b"esds", "missing AudioSpecificConfig"));
    }
    let mut unknown_boxes = Vec::new();
    for child in children {
        reject_encryption_extension(child.kind, child.payload)?;
        if child.kind != BoxType::Normal(*b"sinf") && child.kind != BoxType::Normal(*b"esds") {
            unknown_boxes.push(UnknownBox::decode(child.bytes)?.0);
        }
    }
    Ok((
        Mp4aBox {
            audio,
            esds_box,
            unknown_boxes,
        },
        encryption,
    ))
}

fn parse_tenc(
    payload: &[u8],
    scheme: EncryptionScheme,
) -> Result<(InitializationVector, [u8; 16])> {
    let mut reader = Reader::new(payload, *b"tenc");
    let (version, _) = reader.full_header(1, 0)?;
    if reader.u8()? != 0 {
        return Err(invalid(*b"tenc", "reserved"));
    }
    let pattern = reader.u8()?;
    if pattern != 0 {
        return Err(if version == 0 {
            invalid(*b"tenc", "reserved")
        } else {
            Error::Unsupported(Unsupported::PatternEncryption)
        });
    }
    if reader.u8()? != 1 {
        return Err(Error::Unsupported(Unsupported::UnprotectedTrack));
    }
    let iv_size = reader.u8()?;
    let mut key_id = [0; 16];
    key_id.copy_from_slice(reader.take(16)?);
    let initialization_vector = match iv_size {
        8 | 16 => InitializationVector::PerSample(iv_size),
        0 if scheme == EncryptionScheme::Cbcs => {
            let constant_size = reader.u8()?;
            if constant_size != 16 {
                return Err(Error::Unsupported(Unsupported::IvSize(constant_size)));
            }
            let mut iv = [0; 16];
            iv.copy_from_slice(reader.take(16)?);
            InitializationVector::Constant(iv)
        }
        0 => return Err(Error::Unsupported(Unsupported::ConstantIv)),
        value => return Err(Error::Unsupported(Unsupported::IvSize(value))),
    };
    reader.finish()?;
    Ok((initialization_vector, key_id))
}
