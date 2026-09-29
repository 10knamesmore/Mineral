//! Probe audio tags and media facts without modifying source files.

use std::{
    borrow::Cow,
    num::{NonZeroU8, NonZeroU32},
    path::{Path, PathBuf},
    time::Duration,
};

use lofty::{
    file::{AudioFile, FileType, TaggedFileExt},
    prelude::Accessor,
    tag::ItemKey,
};
use mineral_model::{AudioFormat, Lyrics};
use sha2::{Digest, Sha256};

use super::{lyrics, mp4_duration};
use crate::error::{Error, Result as LocalResult};

/// Media facts retained with each scanned file.
#[derive(Debug)]
pub(crate) struct Metadata {
    /// Encoded file size in bytes when opened for probing.
    pub(crate) size: u64,

    /// Tag title, falling back to the filename stem.
    pub(crate) name: String,

    /// Ordered track artists.
    pub(crate) artists: Vec<String>,

    /// Album title, when tagged.
    pub(crate) album: Option<String>,

    /// Album artists used to identify the album.
    pub(crate) album_artists: Vec<String>,

    /// Measured duration.
    pub(crate) duration: Option<Duration>,

    /// Actual encoded bitrate.
    pub(crate) bitrate_bps: Option<NonZeroU32>,

    /// Container/codec classification.
    pub(crate) format: AudioFormat,

    /// PCM bit depth where known.
    pub(crate) bit_depth: Option<NonZeroU8>,

    /// Extracted embedded cover. Directory covers are resolved on each projection.
    pub(crate) embedded_cover: Option<PathBuf>,

    /// Parsed embedded lyrics, used when no same-name LRC exists.
    pub(crate) embedded_lyrics: Option<Lyrics>,
}

impl Metadata {
    /// Convert to the common song model's millisecond representation.
    pub(crate) fn duration_ms(&self) -> Result<Option<u64>, std::num::TryFromIntError> {
        self.duration
            .map(|duration| duration.as_millis().try_into())
            .transpose()
    }
}

/// Recognized audio extensions. Content probing still validates the actual format.
pub(crate) fn supported(path: &Path) -> bool {
    path.extension().and_then(FileType::from_ext).is_some()
}

/// Probe a recognized file. Malformed audio is a scan error, not a silently removed song.
pub(crate) fn probe(path: &Path, cover_dir: &Path) -> LocalResult<Metadata> {
    let mut file = std::fs::File::open(path).map_err(|source| Error::File {
        operation: "open local audio",
        path: path.to_owned(),
        source,
    })?;
    let size = file
        .metadata()
        .map_err(|source| Error::File {
            operation: "read local audio size",
            path: path.to_owned(),
            source,
        })?
        .len();
    // Container signatures take precedence over extensions, such as MP4 saved as .aac.
    let tagged = lofty::read_from(&mut file).map_err(|source| Error::Probe {
        path: path.to_owned(),
        source,
    })?;
    let properties = tagged.properties();
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let bitrate_bps = properties
        .audio_bitrate()
        .and_then(|value| value.checked_mul(1000))
        .and_then(NonZeroU32::new);
    let bit_depth = properties.bit_depth().and_then(NonZeroU8::new);
    let lofty_duration = (!properties.duration().is_zero()).then(|| properties.duration());
    let embedded_lyrics = lyrics::read(&tagged, path);
    let (format, duration) = match tagged.file_type() {
        FileType::Mpeg => (AudioFormat::Mp3, lofty_duration),
        FileType::Aac => (AudioFormat::Aac, lofty_duration),
        FileType::Flac => (AudioFormat::Flac, lofty_duration),
        FileType::Wav => (AudioFormat::Wav, lofty_duration),
        FileType::Ape => (AudioFormat::Ape, lofty_duration),
        FileType::Vorbis => (AudioFormat::Ogg, lofty_duration),
        // MP4 may contain AAC or ALAC; the container does not identify the codec.
        FileType::Mp4 => {
            let duration = match mp4_duration::read(file, path) {
                Ok(Some(duration)) => Some(duration),
                Ok(None) => {
                    mineral_log::warn!(target: "local_library", path = %path.display(), "MP4 audio duration unavailable");
                    None
                }
                Err(error) => {
                    mineral_log::warn!(target: "local_library", path = %path.display(), error = mineral_log::chain(&error), "MP4 duration probe failed");
                    None
                }
            };
            (AudioFormat::Mp4, duration)
        }
        other => (
            AudioFormat::Other(format!("{other:?}").to_lowercase()),
            lofty_duration,
        ),
    };
    let embedded_cover = tag
        .and_then(|tag| tag.pictures().first())
        .map(|picture| -> LocalResult<_> {
            std::fs::create_dir_all(cover_dir).map_err(|source| Error::File {
                operation: "create embedded cover directory",
                path: cover_dir.to_owned(),
                source,
            })?;
            let file = cover_dir.join(hex::encode(Sha256::digest(picture.data())));
            if !file.try_exists().map_err(|source| Error::File {
                operation: "check embedded cover",
                path: file.clone(),
                source,
            })? {
                let temporary = cover_dir.join(uuid::Uuid::new_v4().to_string());
                std::fs::write(&temporary, picture.data()).map_err(|source| Error::File {
                    operation: "write embedded cover",
                    path: temporary.clone(),
                    source,
                })?;
                std::fs::rename(&temporary, &file).map_err(|source| Error::File {
                    operation: "publish embedded cover",
                    path: file.clone(),
                    source,
                })?;
            }
            Ok(file)
        })
        .transpose()?;
    let name = tag
        .and_then(Accessor::title)
        .map(Cow::into_owned)
        .or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .ok_or_else(|| Error::MissingFilename {
            path: path.to_owned(),
        })?;
    Ok(Metadata {
        size,
        name,
        artists: tag
            .map(|tag| {
                tag.get_strings(&ItemKey::TrackArtist)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        album: tag.and_then(Accessor::album).map(Cow::into_owned),
        album_artists: tag
            .map(|tag| {
                tag.get_strings(&ItemKey::AlbumArtist)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        duration,
        bitrate_bps,
        format,
        bit_depth,
        embedded_cover,
        embedded_lyrics,
    })
}

/// Embedded art takes precedence; conventional directory images need no extra audio probe.
pub(crate) fn cover(path: &Path, metadata: &Metadata) -> Option<PathBuf> {
    metadata.embedded_cover.clone().or_else(|| {
        path.parent().and_then(|directory| {
            ["cover.jpg", "cover.png", "folder.jpg", "folder.png"]
                .into_iter()
                .map(|name| directory.join(name))
                .find(|path| path.is_file())
        })
    })
}
