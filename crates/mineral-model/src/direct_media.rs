//! Direct media access and source-neutral playback facts.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::{AudioFormat, BitRate, MediaUrl, SongId};

/// Container layout governing the cost of random access while opening a decoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StreamLayout {
    /// A contiguous resource such as MP3 or FLAC with inexpensive random access.
    #[default]
    Contiguous,

    /// A segmented container whose complete seek index requires scanning every segment.
    Chunked,
}

/// A directly accessible remote resource.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteLocator {
    /// Remote resource URL.
    url: Url,

    /// Request headers required whenever the resource is opened or reopened.
    headers: Vec<(String, String)>,
}

impl RemoteLocator {
    /// Creates a remote locator.
    ///
    /// # Params:
    ///   - `url`: Remote resource URL.
    ///   - `headers`: Request headers required by the resource.
    #[must_use]
    fn new(url: Url, headers: Vec<(String, String)>) -> Self {
        Self { url, headers }
    }

    /// Returns the remote URL.
    #[must_use]
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Returns the request headers.
    #[must_use]
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }
}

/// The direct access location of media, independent of its source identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectLocator {
    /// A remote URL and its request headers.
    Remote(RemoteLocator),

    /// A local filesystem path.
    Local(PathBuf),
}

impl DirectLocator {
    /// Projects the locator into the shared remote-or-local URL vocabulary.
    #[must_use]
    pub fn media_url(&self) -> MediaUrl {
        match self {
            Self::Remote(remote) => MediaUrl::Remote(remote.url.clone()),
            Self::Local(path) => MediaUrl::Local(path.clone()),
        }
    }

    /// Returns the local path when this locator is local.
    #[must_use]
    pub fn local_path(&self) -> Option<&Path> {
        match self {
            Self::Remote(_) => None,
            Self::Local(path) => Some(path),
        }
    }

    /// Returns the remote locator when this locator is remote.
    #[must_use]
    pub fn remote(&self) -> Option<&RemoteLocator> {
        match self {
            Self::Remote(remote) => Some(remote),
            Self::Local(_) => None,
        }
    }
}

/// Source-neutral media facts available to playback and presentation consumers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackMediaInfo {
    /// Song identity, including its source namespace.
    pub song_id: SongId,

    /// Actual bitrate in bits per second, or `None` when unknown.
    pub bitrate_bps: Option<u32>,

    /// Normalized quality delivered for the request.
    pub quality: BitRate,

    /// Encoded resource size in bytes, or `None` when unknown.
    pub size: Option<u64>,

    /// Container or codec format, or `None` when unavailable.
    pub format: Option<AudioFormat>,

    /// Bits per sample when meaningful and known.
    pub bit_depth: Option<u8>,

    /// Whether a hook replaced the source-provided media.
    #[serde(default)]
    pub substituted: bool,
}

/// Optional direct access to media plus the facts known before it is opened.
///
/// Direct access supports hooks and copy/export presentation. Its absence does not make a prepared
/// playback unplayable; providers may open and prepare media without exposing a URL or path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectMedia {
    /// Direct access location.
    locator: DirectLocator,

    /// Source-neutral facts known before opening.
    info: PlaybackMediaInfo,

    /// Container access layout used by direct openers.
    #[serde(default)]
    layout: StreamLayout,
}

impl DirectMedia {
    /// Creates directly accessible remote media.
    ///
    /// # Params:
    ///   - `info`: Source-neutral facts known before opening.
    ///   - `url`: Remote resource URL.
    ///   - `headers`: Request headers required by the resource.
    ///   - `layout`: Container access layout.
    #[must_use]
    pub fn remote(
        info: PlaybackMediaInfo,
        url: Url,
        headers: Vec<(String, String)>,
        layout: StreamLayout,
    ) -> Self {
        Self {
            locator: DirectLocator::Remote(RemoteLocator::new(url, headers)),
            info,
            layout,
        }
    }

    /// Creates directly accessible local media.
    ///
    /// # Params:
    ///   - `info`: Source-neutral facts known before opening.
    ///   - `path`: Local media path.
    #[must_use]
    pub fn local(info: PlaybackMediaInfo, path: PathBuf) -> Self {
        Self {
            locator: DirectLocator::Local(path),
            info,
            layout: StreamLayout::Contiguous,
        }
    }

    /// Returns the direct locator.
    #[must_use]
    pub fn locator(&self) -> &DirectLocator {
        &self.locator
    }

    /// Returns the source-neutral facts known before opening.
    #[must_use]
    pub fn info(&self) -> &PlaybackMediaInfo {
        &self.info
    }

    /// Returns the container access layout.
    #[must_use]
    pub fn layout(&self) -> StreamLayout {
        self.layout
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        AudioFormat, BitRate, DirectMedia, PlaybackMediaInfo, SongId, SourceKind, StreamLayout,
    };

    /// Required remote headers survive serialization with the direct locator.
    #[test]
    fn stream_headers_survive_serde_roundtrip() -> color_eyre::Result<()> {
        let media = DirectMedia::remote(
            PlaybackMediaInfo {
                song_id: SongId::new(SourceKind::NETEASE, "1"),
                bitrate_bps: Some(320_000),
                quality: BitRate::Exhigh,
                size: None,
                format: Some(AudioFormat::Mp3),
                bit_depth: None,
                substituted: false,
            },
            "https://example.com/a.m4s".parse()?,
            vec![("Referer".to_owned(), "https://www.bilibili.com".to_owned())],
            StreamLayout::Contiguous,
        );
        let back = serde_json::from_str::<DirectMedia>(&serde_json::to_string(&media)?)?;
        assert_eq!(
            back.locator().remote().map(RemoteLocator::headers),
            media.locator().remote().map(RemoteLocator::headers)
        );
        Ok(())
    }

    use crate::RemoteLocator;

    /// Stream layout defaults to contiguous access.
    #[test]
    fn stream_layout_defaults_contiguous() {
        assert_eq!(StreamLayout::default(), StreamLayout::Contiguous);
    }

    /// Segmented layout survives serialization.
    #[test]
    fn layout_survives_serde_roundtrip() -> color_eyre::Result<()> {
        let media = DirectMedia::remote(
            PlaybackMediaInfo {
                song_id: SongId::new(SourceKind::BILIBILI, "BV1x:1"),
                bitrate_bps: Some(192_000),
                quality: BitRate::Exhigh,
                size: None,
                format: Some(AudioFormat::Aac),
                bit_depth: None,
                substituted: false,
            },
            "https://example.com/a.m4s".parse()?,
            Vec::new(),
            StreamLayout::Chunked,
        );
        let back = serde_json::from_str::<DirectMedia>(&serde_json::to_string(&media)?)?;
        assert_eq!(back.layout(), StreamLayout::Chunked);
        Ok(())
    }
}
