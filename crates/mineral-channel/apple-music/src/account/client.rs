//! Authenticated HTTP requests and lazy account validation.
//! Endpoint-specific metadata and media policies belong to their callers.

use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, COOKIE, HeaderMap, HeaderValue, ORIGIN, REFERER};
use reqwest::{Client, Proxy, RequestBuilder};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;

use super::credentials::{self, StoredAppleMusicAuth, WriteLock};
use super::developer_token::{self, DeveloperToken};
use super::response::checked_response;
use crate::{AppleMusicConfig, Error, Result};

/// One account's connection pool and cached authentication, shared by channel operations.
/// Construction is local; the first authenticated request acquires a developer token.
pub(crate) struct AccountClient {
    http: Client,

    account: Mutex<Option<AccountInfo>>,

    developer_token: Mutex<Option<Arc<DeveloperToken>>>,

    auth: StoredAppleMusicAuth,

    developer_token_cache_ttl: Duration,
}

/// Validated subscription information, independent of the website token's lifetime.
#[derive(Clone)]
pub(crate) struct AccountInfo {
    /// Subscription storefront used for catalog lookups.
    pub storefront: String,
}

#[derive(Deserialize)]
struct AccountResponse {
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

impl AccountClient {
    /// Builds local transport state without making a network request.
    pub(crate) fn new(config: &AppleMusicConfig, auth: StoredAppleMusicAuth) -> Result<Arc<Self>> {
        let mut builder = Client::builder()
            .user_agent(concat!("Mineral/", env!("CARGO_PKG_VERSION")))
            .no_proxy()
            .timeout(Duration::from_secs(*config.timeout_secs()));
        if let Some(proxy) = config.proxy() {
            builder = builder.proxy(Proxy::all(proxy).map_err(Error::network)?);
        }
        Ok(Arc::new(Self {
            http: builder.build().map_err(Error::network)?,
            account: Mutex::new(None),
            developer_token: Mutex::new(None),
            auth,
            developer_token_cache_ttl: *config.developer_token_cache_ttl(),
        }))
    }

    /// The pool has no default account headers; CDN requests can reuse it without credentials.
    pub(crate) fn http(&self) -> &Client {
        &self.http
    }

    /// Verify the account once; a developer-token refresh does not expire its storefront.
    pub(crate) async fn account(&self) -> Result<AccountInfo> {
        let mut cached = self.account.lock().await;
        if let Some(account) = cached.as_ref() {
            return Ok(account.clone());
        }
        mineral_log::info!(target: "apple_music", "validating account session");
        let response: AccountResponse = self
            .authenticated_json(
                || {
                    self.http
                        .get("https://amp-api.music.apple.com/v1/me/account?meta=subscription")
                },
                |_| false,
            )
            .await?;
        let storefront = response
            .meta
            .subscription
            .ok_or(Error::SubscriptionUnavailable)?
            .storefront;
        mineral_log::info!(target: "apple_music", %storefront, "account session validated");
        let account = AccountInfo { storefront };
        *cached = Some(account.clone());
        Ok(account)
    }

    /// HTTP 401/403 and endpoint-specific session errors share one refresh/replay budget.
    /// The request factory must recreate the same operation for the second attempt.
    pub(crate) async fn authenticated_json<T: DeserializeOwned>(
        &self,
        request: impl Fn() -> RequestBuilder,
        authorization_rejected: fn(&T) -> bool,
    ) -> Result<T> {
        let token = self.current_developer_token(None).await?;
        let result = self
            .authenticated_attempt(&request, &token, authorization_rejected)
            .await;
        if !matches!(&result, Err(Error::AuthRequired)) {
            return result;
        }
        let token = self.current_developer_token(Some(&token)).await?;
        self.authenticated_attempt(&request, &token, authorization_rejected)
            .await
    }

    async fn authenticated_attempt<T: DeserializeOwned>(
        &self,
        request: &impl Fn() -> RequestBuilder,
        token: &DeveloperToken,
        authorization_rejected: fn(&T) -> bool,
    ) -> Result<T> {
        let response = request()
            .headers(self.headers(&token.value)?)
            .send()
            .await
            .map_err(Error::network)?;
        let bytes = checked_response(response)?
            .bytes()
            .await
            .map_err(Error::network)?;
        let body = serde_json::from_slice(&bytes)?;
        if authorization_rejected(&body) {
            return Err(Error::AuthRequired);
        }
        Ok(body)
    }

    /// Serialize refreshes within this client and across processes. The failed Arc
    /// identifies the acquisition, so a late rejection cannot invalidate a newer token
    /// even when the website publishes the same JWT again.
    async fn current_developer_token(
        &self,
        rejected: Option<&Arc<DeveloperToken>>,
    ) -> Result<Arc<DeveloperToken>> {
        let mut cached = self.developer_token.lock().await;
        let ttl = self.developer_token_cache_ttl;
        if let Some(token) = cached.as_ref()
            && token.is_valid(ttl)?
            && rejected.is_none_or(|failed| !Arc::ptr_eq(failed, token))
        {
            return Ok(Arc::clone(token));
        }

        let lock = WriteLock::acquire().await?;
        let mut credentials = credentials::read()?;
        if let Some(token) = credentials.developer_token.as_ref()
            && token.is_valid(ttl)?
            && rejected.is_none_or(|failed| failed.as_ref() != token)
        {
            let token = Arc::new(token.clone());
            *cached = Some(Arc::clone(&token));
            mineral_log::debug!(target: "apple_music", "developer token loaded from disk");
            return Ok(token);
        }

        let token = developer_token::fetch(&self.http).await?;
        credentials.developer_token = Some(token.clone());
        // Read/merge/write under the stable lock preserves a newer explicit login.
        lock.save(&credentials)?;
        let token = Arc::new(token);
        *cached = Some(Arc::clone(&token));
        Ok(token)
    }

    /// Account headers are attached per API request and marked sensitive.
    fn headers(&self, developer_token: &str) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        for (name, text) in [
            (AUTHORIZATION, format!("Bearer {developer_token}")),
            (
                COOKIE,
                format!("media-user-token={}", self.auth.media_user_token),
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
}
