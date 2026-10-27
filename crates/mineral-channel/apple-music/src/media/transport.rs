use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hls_m3u8::MediaPlaylist;
use hls_m3u8::types::{ByteRange, KeyFormat};
use reqwest::header::{CONTENT_TYPE, RANGE};
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::fs;
use url::Url;
use zeroize::Zeroizing;

use super::PlaybackIdentity;
use super::error::{Error, Result};
use super::license::{ContentKey, DeviceIdentity, LicenseExchange, key_initialization};
use crate::api::Api;

/// iTunes can reject authentication inside an HTTP 200 response.
const SESSION_ENDED_FAILURE: &str = "2002";

#[derive(Clone)]
pub(super) struct Part {
    url: Url,

    range: Option<Range<usize>>,
}

impl Part {
    pub(super) fn resource_offset(&self) -> Result<u64> {
        self.range
            .as_ref()
            .map_or(Ok(0), |range| Ok(u64::try_from(range.start)?))
    }
}

pub(super) struct Manifest {
    pub initialization: Part,

    pub segments: Vec<MediaSegment>,

    pub key_uri: String,
}

pub(super) struct MediaSegment {
    pub source: Part,

    /// EXTINF duration locates seek candidates; fragment samples supply exact timing.
    pub duration: Duration,
}

pub(super) struct PlaybackAsset {
    pub url: Url,

    /// Bitrate declared for the selected rendition, converted from metadata.bitRate.
    pub bitrate_bps: Option<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AssetRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    salable_adam_id: Option<&'a str>,

    #[serde(skip_serializing_if = "Option::is_none")]
    universal_library_id: Option<&'a str>,

    language: &'static str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssetResponse {
    #[serde(default)]
    song_list: Vec<PlaybackSong>,

    failure_type: Option<String>,
}

#[derive(Deserialize)]
struct PlaybackSong {
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    flavor: Option<String>,

    #[serde(rename = "URL")]
    url: String,

    metadata: Option<AssetMetadata>,
}

#[derive(Deserialize)]
struct AssetMetadata {
    #[serde(rename = "bitRate")]
    bitrate_kbps: Option<u32>,
}

#[derive(Serialize)]
struct LicenseRequest<'a> {
    challenge: String,

    #[serde(rename = "key-system")]
    key_system: &'static str,

    uri: &'a str,

    #[serde(rename = "adamId")]
    track_id: &'a str,

    #[serde(rename = "isLibrary")]
    is_library: bool,

    #[serde(rename = "user-initiated")]
    user_initiated: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LicenseResponse {
    status: i32,

    license: Option<String>,

    failure_type: Option<String>,
}

pub(super) async fn asset(api: &Api, identity: &PlaybackIdentity) -> Result<PlaybackAsset> {
    let (salable_adam_id, universal_library_id) = match identity {
        PlaybackIdentity::Catalog(id) => (Some(id.as_str()), None),
        PlaybackIdentity::Library(id) => (None, Some(id.as_str())),
    };
    let body = AssetRequest {
        salable_adam_id,
        universal_library_id,
        language: "en-US",
    };
    let body = serde_json::to_vec(&body)?;
    let response = api
        .authenticated_json(
            || {
                api.client
                    .post("https://play.itunes.apple.com/WebObjects/MZPlay.woa/wa/webPlayback")
                    .header(CONTENT_TYPE, "application/json")
                    .body(body.clone())
            },
            |response: &AssetResponse| {
                response.failure_type.as_deref() == Some(SESSION_ENDED_FAILURE)
            },
        )
        .await?;
    if response.failure_type.is_some() {
        return Err(Error::PlaybackRejected);
    }
    let song = response
        .song_list
        .into_iter()
        .next()
        .ok_or(Error::NoRendition)?;
    let asset = song
        .assets
        .into_iter()
        .find(|asset| {
            matches!(identity, PlaybackIdentity::Library(_))
                || asset.flavor.as_deref() == Some("28:ctrp256")
        })
        .ok_or(Error::NoRendition)?;
    let bitrate_bps = asset
        .metadata
        .and_then(|metadata| metadata.bitrate_kbps)
        .map(|kbps| kbps.checked_mul(1000).ok_or(Error::Overflow))
        .transpose()?;
    Ok(PlaybackAsset {
        url: Url::parse(&asset.url)?,
        bitrate_bps,
    })
}

pub(super) async fn manifest(client: &Client, base: &Url) -> Result<Manifest> {
    let response = cdn_response(
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
    let key_uri = first
        .keys
        .iter()
        .filter_map(|key| key.as_ref())
        .find(|key| {
            matches!(
                key.format,
                Some(KeyFormat::Widevine) | None | Some(KeyFormat::Identity)
            )
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
            source: Part {
                url: base.join(segment.uri())?,
                range: byte_range(segment.byte_range.map(|range| *range))?,
            },
            duration: segment.duration.duration(),
        });
    }
    Ok(Manifest {
        initialization: Part {
            url: base.join(map.uri())?,
            range: byte_range(map.range())?,
        },
        segments,
        key_uri,
    })
}

pub(super) async fn content_keys(api: &Api, track_id: &str, uri: &str) -> Result<Vec<ContentKey>> {
    let started = Instant::now();
    let device = match api.config.device_file() {
        Some(path) => {
            let bytes = Zeroizing::new(fs::read(path).await?);
            DeviceIdentity::from_wvd(&bytes)?
        }
        None => DeviceIdentity::from_wvd(include_bytes!("../../assets/gamdl-android-l3.wvd"))?,
    };
    let device = Arc::new(device);
    let data = STANDARD.decode(
        uri.rsplit(',')
            .next()
            .ok_or(Error::InvalidMedia { field: "key URI" })?,
    )?;
    let initialization = match <[u8; 16]>::try_from(data.as_slice()) {
        Ok(key_id) => key_initialization(&key_id),
        Err(_) => data,
    };
    let exchange = LicenseExchange::streaming(device, initialization)?;
    let body = LicenseRequest {
        challenge: STANDARD.encode(exchange.challenge()),
        key_system: "com.widevine.alpha",
        uri,
        track_id,
        is_library: false,
        user_initiated: true,
    };
    let body = serde_json::to_vec(&body)?;
    let response = api
        .authenticated_json(
            || {
                api.client
                    .post("https://play.itunes.apple.com/WebObjects/MZPlay.woa/wa/acquireWebPlaybackLicense")
                    .header(CONTENT_TYPE, "application/json")
                    .body(body.clone())
            },
            |response: &LicenseResponse| response.failure_type.as_deref() == Some(SESSION_ENDED_FAILURE),
        )
        .await?;
    if response.status != 0 {
        return Err(Error::LicenseRejected {
            status: response.status,
        });
    }
    let encoded = response.license.ok_or(Error::InvalidMedia {
        field: "license payload",
    })?;
    let keys = exchange.finish(&STANDARD.decode(encoded)?)?;
    mineral_log::debug!(target: "apple_music", elapsed_ms = started.elapsed().as_millis(), "license exchange completed");
    Ok(keys)
}

pub(super) async fn download(client: &Client, part: &Part) -> Result<Vec<u8>> {
    let mut request = client.get(part.url.clone());
    if let Some(range) = &part.range {
        request = request.header(RANGE, range_header(range)?);
    }
    let response = cdn_response(request.send().await.map_err(Error::network)?)?;
    if part.range.is_some() && response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(Error::ByteRange);
    }
    let bytes = response.bytes().await.map_err(Error::network)?;
    if part
        .range
        .as_ref()
        .is_some_and(|range| bytes.len() != range.len())
    {
        return Err(Error::ByteRange);
    }
    Ok(bytes.to_vec())
}

pub(super) async fn read_header_range(
    client: &Client,
    part: &Part,
    start: usize,
    length: usize,
) -> Result<Vec<u8>> {
    let origin = part.range.as_ref().map_or(0, |range| range.start);
    let start = origin.checked_add(start).ok_or(Error::Overflow)?;
    let end = start.checked_add(length).ok_or(Error::Overflow)?;
    let response = cdn_response(
        client
            .get(part.url.clone())
            .header(RANGE, range_header(&(start..end))?)
            .send()
            .await
            .map_err(Error::network)?,
    )?;
    if response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(Error::ByteRange);
    }
    let bytes = response.bytes().await.map_err(Error::network)?;
    if bytes.len() != length {
        return Err(Error::ByteRange);
    }
    Ok(bytes.to_vec())
}

/// Read a bounded fragment prefix; a short fragment may end before the requested limit.
pub(super) async fn read_header_prefix(
    client: &Client,
    part: &Part,
    limit: NonZeroUsize,
) -> Result<Vec<u8>> {
    let start = part.range.as_ref().map_or(0, |range| range.start);
    let length = part
        .range
        .as_ref()
        .map_or(limit.get(), |range| limit.get().min(range.len()));
    let end = start.checked_add(length).ok_or(Error::Overflow)?;
    let response = cdn_response(
        client
            .get(part.url.clone())
            .header(RANGE, range_header(&(start..end))?)
            .send()
            .await
            .map_err(Error::network)?,
    )?;
    if response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(Error::ByteRange);
    }
    let bytes = response.bytes().await.map_err(Error::network)?;
    if bytes.len() > length {
        return Err(Error::ByteRange);
    }
    Ok(bytes.to_vec())
}

fn range_header(range: &Range<usize>) -> Result<String> {
    let end = range.end.checked_sub(1).ok_or(Error::ByteRange)?;
    Ok(format!("bytes={}-{end}", range.start))
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

fn cdn_response(response: Response) -> Result<Response> {
    if !response.status().is_success() {
        return Err(Error::Http {
            status: response.status().as_u16(),
        });
    }
    Ok(response)
}
