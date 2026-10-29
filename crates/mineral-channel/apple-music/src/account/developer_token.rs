//! Fetches the public website token and checks its JWT expiry and configured cache age.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::response::checked_response;
use crate::{Error, Result};

/// Website token and its acquisition/expiry times, in Unix seconds.
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct DeveloperToken {
    pub value: String,

    pub fetched_at_unix_secs: u64,

    /// JWT exp; cache freshness also applies the currently configured TTL.
    pub expires_at_unix_secs: u64,
}

impl DeveloperToken {
    pub(super) fn is_valid(&self, ttl: Duration) -> Result<bool> {
        let age_limit =
            self.fetched_at_unix_secs
                .checked_add(ttl.as_secs())
                .ok_or(Error::InvalidData {
                    field: "developer token cache TTL",
                })?;
        Ok(now()? < self.expires_at_unix_secs.min(age_limit))
    }
}

#[derive(Deserialize)]
struct TokenClaims {
    exp: u64,
}

fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

/// Fetches the public website token without sending account credentials.
pub(super) async fn fetch(client: &Client) -> Result<DeveloperToken> {
    let homepage = text(client, "https://music.apple.com").await?;
    let asset_pattern = Regex::new(r#"/(assets/index[~-][^/\"]+\.js)"#)?;
    let asset = asset_pattern
        .captures(&homepage)
        .and_then(|captures| captures.get(1))
        .ok_or(Error::InvalidData {
            field: "website asset",
        })?;
    let javascript = text(
        client,
        &format!("https://music.apple.com/{}", asset.as_str()),
    )
    .await?;
    let token_pattern =
        Regex::new(r#"\"(eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+)\""#)?;
    let token = token_pattern
        .captures(&javascript)
        .and_then(|captures| captures.get(1))
        .ok_or(Error::InvalidData {
            field: "website developer token",
        })?
        .as_str();
    let claims = token.split('.').nth(1).ok_or(Error::InvalidData {
        field: "developer token claims",
    })?;
    let claims: TokenClaims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims)?)?;
    let fetched_at_unix_secs = now()?;
    let expires_at_unix_secs = claims.exp;
    if expires_at_unix_secs <= fetched_at_unix_secs {
        return Err(Error::InvalidData {
            field: "expired website developer token",
        });
    }
    mineral_log::info!(target: "apple_music", expires_at_unix_secs, "developer token fetched");
    Ok(DeveloperToken {
        value: token.to_owned(),
        fetched_at_unix_secs,
        expires_at_unix_secs,
    })
}

async fn text(client: &Client, url: &str) -> Result<String> {
    let response = client.get(url).send().await.map_err(Error::network)?;
    checked_response(response)?
        .text()
        .await
        .map_err(Error::network)
}
