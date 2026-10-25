//! Website developer credentials and subscription storefront discovery.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use regex::Regex;
use reqwest::header::{AUTHORIZATION, COOKIE, HeaderMap, HeaderValue, ORIGIN, REFERER};
use reqwest::{Client, Response};
use serde::Deserialize;

use crate::credential::{DeveloperToken, now};
use crate::{Error, Result, StoredAppleMusicAuth};

/// Account storefront is independent of the website developer-token lifetime.
#[derive(Clone)]
pub(crate) struct Session {
    /// Subscription storefront used for catalog lookups.
    pub storefront: String,
}

#[derive(Deserialize)]
struct TokenClaims {
    exp: u64,
}

#[derive(Deserialize)]
pub(crate) struct Account {
    meta: AccountMeta,
}

#[derive(Deserialize)]
struct AccountMeta {
    subscription: Option<Subscription>,
}

#[derive(Deserialize)]
struct Subscription {
    storefront: String,
}

/// Fetches the public website token without sending account credentials.
pub(crate) async fn developer_token(client: &Client) -> Result<DeveloperToken> {
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

/// Decodes an already authenticated account response.
pub(crate) fn account(account: Account) -> Result<Session> {
    let storefront = account
        .meta
        .subscription
        .ok_or(Error::AuthRequired)?
        .storefront;
    mineral_log::info!(target: "apple_music", %storefront, "account session validated");
    Ok(Session { storefront })
}

/// Account headers are attached only to Apple API requests and marked sensitive.
pub(crate) fn headers(auth: &StoredAppleMusicAuth, developer_token: &str) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for (name, text) in [
        (AUTHORIZATION, format!("Bearer {developer_token}")),
        (
            COOKIE,
            format!("media-user-token={}", auth.media_user_token),
        ),
    ] {
        let mut value = HeaderValue::from_str(&text)?;
        value.set_sensitive(true);
        headers.insert(name, value);
    }
    headers.insert(ORIGIN, HeaderValue::from_static("https://music.apple.com"));
    headers.insert(
        REFERER,
        HeaderValue::from_static("https://music.apple.com/"),
    );
    Ok(headers)
}

/// Classifies failures without retaining response bodies or request URLs.
pub(crate) fn checked(response: Response) -> Result<Response> {
    let status = response.status();
    if matches!(status.as_u16(), 401 | 403) {
        return Err(Error::AuthRequired);
    }
    if !status.is_success() {
        return Err(Error::Http {
            status: status.as_u16(),
        });
    }
    Ok(response)
}

async fn text(client: &Client, url: &str) -> Result<String> {
    let response = client.get(url).send().await.map_err(Error::network)?;
    checked(response)?.text().await.map_err(Error::network)
}
