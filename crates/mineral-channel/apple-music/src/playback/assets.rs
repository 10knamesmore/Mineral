//! Selects the existing catalog AAC rendition or the server-selected library upload.

use mineral_model::BitRate;
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};
use url::Url;

use super::error::{Error, Result};
use super::hls;
use crate::account::{AccountClient, SESSION_ENDED_FAILURE};
use crate::metadata::SongIdentity;

pub(super) enum PlaybackAsset {
    CatalogHls {
        track_id: String,

        url: Url,

        bitrate_bps: Option<u32>,
    },

    LibraryUpload {
        url: Url,

        bitrate_bps: Option<u32>,
    },
}

impl PlaybackAsset {
    /// Source-declared bits/s: web metadata.bitRate or HLS AVERAGE-BANDWIDTH.
    pub fn bitrate_bps(&self) -> Option<u32> {
        match self {
            Self::CatalogHls { bitrate_bps, .. } | Self::LibraryUpload { bitrate_bps, .. } => {
                *bitrate_bps
            }
        }
    }
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

/// Catalog standard selects AAC 128; higher and the remaining preferences use web AAC 256.
/// Unmatched library uploads retain their server-selected direct resource.
pub(super) async fn select(
    account: &AccountClient,
    identity: &SongIdentity,
    quality: BitRate,
) -> Result<PlaybackAsset> {
    if quality == BitRate::Standard
        && let SongIdentity::Catalog(track_id) = identity
    {
        return standard_asset(account, track_id).await;
    }
    let (salable_adam_id, universal_library_id) = match identity {
        SongIdentity::Catalog(id) => (Some(id.as_str()), None),
        SongIdentity::Library(id) => (None, Some(id.as_str())),
    };
    let body = AssetRequest {
        salable_adam_id,
        universal_library_id,
        language: "en-US",
    };
    let body = serde_json::to_vec(&body)?;
    let response = account
        .authenticated_json(
            || {
                account
                    .http()
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
            matches!(identity, SongIdentity::Library(_))
                || asset.flavor.as_deref() == Some("28:ctrp256")
        })
        .ok_or(Error::NoRendition)?;
    let bitrate_bps = asset
        .metadata
        .and_then(|metadata| metadata.bitrate_kbps)
        .map(|kbps| kbps.checked_mul(1000).ok_or(Error::Overflow))
        .transpose()?;
    let url = Url::parse(&asset.url)?;
    Ok(match identity {
        SongIdentity::Catalog(track_id) => PlaybackAsset::CatalogHls {
            track_id: track_id.clone(),
            url,
            bitrate_bps,
        },
        SongIdentity::Library(_) => PlaybackAsset::LibraryUpload { url, bitrate_bps },
    })
}

async fn standard_asset(account: &AccountClient, track_id: &str) -> Result<PlaybackAsset> {
    let response: EnhancedAssetResponse = account
        .authenticated_json(
            || {
                account
                    .http()
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
    let (url, bitrate_bps) = hls::standard_rendition(account.http(), &master_url).await?;
    Ok(PlaybackAsset::CatalogHls {
        track_id: track_id.to_owned(),
        url,
        bitrate_bps,
    })
}
