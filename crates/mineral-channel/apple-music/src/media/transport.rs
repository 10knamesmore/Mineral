use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hls_m3u8::tags::VariantStream;
use hls_m3u8::types::{ByteRange, KeyFormat};
use hls_m3u8::{MasterPlaylist, MediaPlaylist};
use mineral_model::BitRate;
use reqwest::header::{CONTENT_TYPE, RANGE};
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize};
use shiguredo_mp4::{BoxHeader, BoxType, Decode};
use tokio::fs;
use url::Url;
use zeroize::Zeroizing;

use super::PlaybackIdentity;
use super::error::{Error, Result};
use super::license::{
    ContentKey, DeviceIdentity, LicenseExchange, initialization_key_id, key_initialization,
};
use crate::api::Api;

/// iTunes can reject authentication inside an HTTP 200 response.
const SESSION_ENDED_FAILURE: &str = "2002";

// Apple's opening samples use a fixed prefetch key; only the later key needs a license.
const PREFETCH_WIDEVINE_URI: &str = concat!(
    "data:text/plain;base64,AAAAOHBzc2gAAAAA7e+LqXnWSs6jyCfc1R0h7QAAABgSEAAAAAA",
    "AAAAAczEvZTEgICBI88aJmwY=",
);

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

    /// Source-declared bits/s: web metadata.bitRate or HLS AVERAGE-BANDWIDTH.
    pub bitrate_bps: Option<u32>,
}

#[derive(Deserialize)]
struct EnhancedAssetResponse {
    results: EnhancedAssetResults,
}

#[derive(Deserialize)]
struct EnhancedAssetResults {
    assets: Vec<EnhancedAsset>,
}

#[derive(Deserialize)]
struct EnhancedAsset {
    url: Option<String>,
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

/// Catalog standard selects AAC 128; higher and the remaining preferences use web AAC 256.
/// Unmatched library uploads retain their server-selected direct resource.
pub(super) async fn asset(
    api: &Api,
    identity: &PlaybackIdentity,
    quality: BitRate,
) -> Result<PlaybackAsset> {
    if quality == BitRate::Standard
        && let PlaybackIdentity::Catalog(track_id) = identity
    {
        return standard_asset(api, track_id).await;
    }
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

async fn standard_asset(api: &Api, track_id: &str) -> Result<PlaybackAsset> {
    let response: EnhancedAssetResponse = api
        .authenticated_json(
            || {
                api.client
                    .get("https://amp-api.music.apple.com/v1/play/assets")
                    .query(&[
                        ("id", track_id),
                        ("kind", "song"),
                        ("includeLicenseUrls", "true"),
                        ("hlsEncryption", "CBC"),
                        ("hlsProfile", "enhancedHls"),
                    ])
            },
            |_| false,
        )
        .await?;
    let master_url = response
        .results
        .assets
        .into_iter()
        .find_map(|asset| asset.url)
        .ok_or(Error::NoRendition)?;
    let master_url = Url::parse(&master_url)?;
    let response = cdn_response(
        api.client
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
    Ok(PlaybackAsset {
        url: master_url.join(uri)?,
        bitrate_bps: bandwidth.map(u32::try_from).transpose()?,
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

pub(super) async fn content_key(api: &Api, track_id: &str, uri: &str) -> Result<ContentKey> {
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
    let initialization = license_initialization(&data)?;
    let key_id = initialization_key_id(&initialization)?;
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
    keys.into_iter()
        .find(|key| key.id.as_slice() == key_id)
        .ok_or(Error::InvalidMedia {
            field: "content key matching HLS license metadata",
        })
}

/// Web keys carry a raw KID; enhanced playlists carry a v0 Widevine PSSH box.
/// The license challenge embeds the PSSH's protobuf payload, without the MP4 box header.
fn license_initialization(data: &[u8]) -> Result<Vec<u8>> {
    if let Ok(key_id) = <&[u8; 16]>::try_from(data) {
        return Ok(key_initialization(key_id));
    }
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
