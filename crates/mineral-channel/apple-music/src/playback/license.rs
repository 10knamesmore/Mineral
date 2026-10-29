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
use super::widevine::{ContentKey, DeviceIdentity, LicenseExchange, LicenseInitialization};
use crate::account::{AccountClient, SESSION_ENDED_FAILURE};

/// Parsed HLS license metadata. Apple's request still needs the original URI,
/// while key selection and challenge construction use its validated initialization.
pub(super) struct LicenseMetadata {
    uri: String,

    initialization: LicenseInitialization,
}

impl LicenseMetadata {
    pub fn parse_uri(uri: &str) -> Result<Self> {
        let encoded = uri.rsplit_once(',').map_or(uri, |(_, encoded)| encoded);
        let data = STANDARD.decode(encoded)?;
        // Web keys carry a raw KID; enhanced playlists carry a v0 Widevine PSSH.
        let initialization = match <[u8; 16]>::try_from(data.as_slice()) {
            Ok(key_id) => LicenseInitialization::from_key_id(key_id),
            Err(_wrong_length) => LicenseInitialization::parse(widevine_pssh_payload(&data)?)?,
        };
        Ok(Self {
            uri: uri.to_owned(),
            initialization,
        })
    }
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

pub(super) async fn content_key(
    account: &AccountClient,
    device_file: Option<&Path>,
    track_id: &str,
    metadata: &LicenseMetadata,
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
    let exchange = LicenseExchange::streaming(device, &metadata.initialization)?;
    let body = LicenseRequest {
        challenge: STANDARD.encode(exchange.challenge()),
        key_system: "com.widevine.alpha",
        uri: &metadata.uri,
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
    let key = exchange.finish(&STANDARD.decode(encoded)?)?;
    mineral_log::debug!(target: "apple_music", elapsed_ms = started.elapsed().as_millis(), "license exchange completed");
    Ok(key)
}
