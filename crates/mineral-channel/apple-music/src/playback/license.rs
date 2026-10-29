//! Exchanges Widevine challenges through Apple's authenticated playback license endpoint.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};
use tokio::fs;
use zeroize::Zeroizing;

use super::error::{Error, Result};
use super::mp4::widevine_pssh_payload;
use super::widevine::{
    ContentKey, DeviceIdentity, LicenseExchange, initialization_key_id, key_initialization,
};
use crate::account::{AccountClient, SESSION_ENDED_FAILURE};

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

pub(super) async fn content_key(
    account: &AccountClient,
    device_file: Option<&Path>,
    track_id: &str,
    uri: &str,
) -> Result<ContentKey> {
    let started = Instant::now();
    let device = match device_file {
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
    let response = account
        .authenticated_json(
            || {
                account.http()
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
    widevine_pssh_payload(data)
}
