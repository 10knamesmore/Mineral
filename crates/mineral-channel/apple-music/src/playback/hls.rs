//! Reads finite HLS playlists and preserves their segment byte ranges and declared timing.

use std::ops::Range;
use std::time::Duration;

use hls_m3u8::tags::VariantStream;
use hls_m3u8::types::{ByteRange, KeyFormat};
use hls_m3u8::{MasterPlaylist, MediaPlaylist};
use reqwest::Client;
use url::Url;

use super::cdn::{self, MediaResource};
use super::error::{Error, Result};

// Apple's opening samples use a fixed prefetch key; only the later key needs a license.
const PREFETCH_WIDEVINE_URI: &str = concat!(
    "data:text/plain;base64,AAAAOHBzc2gAAAAA7e+LqXnWSs6jyCfc1R0h7QAAABgSEAAAAAA",
    "AAAAAczEvZTEgICBI88aJmwY=",
);

pub(super) struct Manifest {
    pub initialization: MediaResource,

    pub segments: Vec<MediaSegment>,

    pub key_uri: String,
}

pub(super) struct MediaSegment {
    pub source: MediaResource,

    /// EXTINF duration locates seek candidates; fragment samples supply exact timing.
    pub duration: Duration,
}

pub(super) async fn standard_rendition(
    client: &Client,
    master_url: &Url,
) -> Result<(Url, Option<u32>)> {
    let response = cdn::response(
        client
            .get(master_url.clone())
            .send()
            .await
            .map_err(Error::network)?,
    )?;
    let source = response.text().await.map_err(Error::network)?;
    let playlist = MasterPlaylist::try_from(source.as_str()).map_err(|_invalid_playlist| {
        Error::InvalidMedia {
            field: "HLS master playlist",
        }
    })?;
    let (uri, bandwidth) = playlist
        .variant_streams
        .iter()
        .find_map(|variant| match variant {
            VariantStream::ExtXStreamInf {
                uri,
                audio: Some(group),
                stream_data,
                ..
            } if group == "audio-stereo-128" => Some((uri, stream_data.average_bandwidth())),
            _ => None,
        })
        .ok_or(Error::NoRendition)?;
    Ok((
        master_url.join(uri)?,
        bandwidth.map(u32::try_from).transpose()?,
    ))
}

pub(super) async fn manifest(client: &Client, base: &Url) -> Result<Manifest> {
    let response = cdn::response(
        client
            .get(base.clone())
            .send()
            .await
            .map_err(Error::network)?,
    )?;
    mineral_log::debug!(target: "apple_music", http_version = ?response.version(), "media manifest response");
    let source = response.text().await.map_err(Error::network)?;
    // ISO-23001-7 names the same CENC key metadata outside this HLS parser's method enum.
    let normalized = source
        .lines()
        .map(
            |line| match line.strip_prefix("#EXT-X-KEY:METHOD=ISO-23001-7,") {
                Some(attributes) => format!("#EXT-X-KEY:METHOD=SAMPLE-AES,{attributes}"),
                None => line.to_owned(),
            },
        )
        .collect::<Vec<_>>()
        .join("\n");
    let playlist = MediaPlaylist::try_from(normalized.as_str()).map_err(|_invalid_playlist| {
        Error::InvalidMedia {
            field: "HLS playlist",
        }
    })?;
    if !playlist.has_end_list {
        return Err(Error::Unsupported {
            layout: "live playlist",
        });
    }
    let first = playlist
        .segments
        .values()
        .next()
        .ok_or(Error::NoRendition)?;
    let map = first.map.as_ref().ok_or(Error::InvalidMedia {
        field: "initialization map",
    })?;
    let key_uri = playlist
        .segments
        .values()
        .flat_map(|segment| &segment.keys)
        .filter_map(|key| key.as_ref())
        .find(|key| {
            matches!(
                key.format,
                Some(KeyFormat::Widevine) | None | Some(KeyFormat::Identity)
            ) && key.uri() != PREFETCH_WIDEVINE_URI
        })
        .ok_or(Error::InvalidMedia {
            field: "license key URI",
        })?
        .uri()
        .to_string();
    let mut segments = Vec::new();
    for (index, segment) in playlist.segments.values().enumerate() {
        if segment.map.as_ref().is_some_and(|next| next != map)
            || (index > 0 && segment.has_discontinuity)
        {
            return Err(Error::Unsupported {
                layout: "changing initialization or discontinuity",
            });
        }
        segments.push(MediaSegment {
            source: MediaResource {
                url: base.join(segment.uri())?,
                range: byte_range(segment.byte_range.map(|range| *range))?,
            },
            duration: segment.duration.duration(),
        });
    }
    Ok(Manifest {
        initialization: MediaResource {
            url: base.join(map.uri())?,
            range: byte_range(map.range())?,
        },
        segments,
        key_uri,
    })
}

fn byte_range(range: Option<ByteRange>) -> Result<Option<Range<usize>>> {
    range
        .map(|range| {
            let start = range.start().ok_or(Error::InvalidMedia {
                field: "byte range start",
            })?;
            Ok(start..range.end())
        })
        .transpose()
}
