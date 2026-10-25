//! Authenticated, read-only AMP requests with refreshable developer credentials.

use std::sync::Arc;
use std::time::Duration;

use mineral_channel_core::{Page, PageResult};
use reqwest::{Client, Proxy, RequestBuilder};
use rustc_hash::FxHashMap;
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;
use url::Url;

use crate::credential::{self, DeveloperToken, WriteLock};
use crate::session::{self, Session};
use crate::wire::{Resource, ResourcePage, SearchResponse};
use crate::{AppleMusicConfig, Error, Result, StoredAppleMusicAuth};

/// One transport and cached account session shared by metadata and playback.
pub(crate) struct Api {
    pub client: Client,

    session: Mutex<Option<Session>>,

    developer_token: Mutex<Option<Arc<DeveloperToken>>>,

    auth: StoredAppleMusicAuth,

    pub config: AppleMusicConfig,
}

impl Api {
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
            client: builder.build().map_err(Error::network)?,
            session: Mutex::new(None),
            developer_token: Mutex::new(None),
            auth,
            config: config.clone(),
        }))
    }

    /// Verify the account once; a developer-token refresh does not expire its storefront.
    pub(crate) async fn session(&self) -> Result<Session> {
        let mut cached = self.session.lock().await;
        if let Some(session) = cached.as_ref() {
            return Ok(session.clone());
        }
        mineral_log::info!(target: "apple_music", "validating account session");
        let response = self
            .authenticated_json(
                || {
                    self.client
                        .get("https://amp-api.music.apple.com/v1/me/account?meta=subscription")
                },
                |_| false,
            )
            .await?;
        let session = session::account(response)?;
        *cached = Some(session.clone());
        Ok(session)
    }

    /// Resolve the current token for each request. HTTP 401/403 and endpoint-specific
    /// session errors share one refresh/replay budget; a second rejection is returned.
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
            .headers(session::headers(&self.auth, &token.value)?)
            .send()
            .await
            .map_err(Error::network)?;
        let bytes = session::checked(response)?
            .bytes()
            .await
            .map_err(Error::network)?;
        let body = serde_json::from_slice(&bytes)?;
        if authorization_rejected(&body) {
            return Err(Error::AuthRequired);
        }
        Ok(body)
    }

    /// Serialize refreshes within this API and across processes. The failed Arc identifies
    /// the exact acquisition, so late failures cannot invalidate a newer in-memory token
    /// even when the website publishes the same JWT again.
    async fn current_developer_token(
        &self,
        rejected: Option<&Arc<DeveloperToken>>,
    ) -> Result<Arc<DeveloperToken>> {
        let mut cached = self.developer_token.lock().await;
        let ttl = *self.config.developer_token_cache_ttl();
        if let Some(token) = cached.as_ref()
            && token.is_valid(ttl)?
            && rejected.is_none_or(|failed| !Arc::ptr_eq(failed, token))
        {
            return Ok(Arc::clone(token));
        }

        let lock = WriteLock::acquire().await?;
        let mut credentials = credential::read()?;
        if let Some(token) = credentials.developer_token.as_ref()
            && token.is_valid(ttl)?
            && rejected.is_none_or(|failed| failed.as_ref() != token)
        {
            let token = Arc::new(token.clone());
            *cached = Some(Arc::clone(&token));
            mineral_log::debug!(target: "apple_music", "developer token loaded from disk");
            return Ok(token);
        }

        let token = session::developer_token(&self.client).await?;
        credentials.developer_token = Some(token.clone());
        // Read/merge/write under the stable lock preserves a newer explicit login.
        lock.save(&credentials)?;
        let token = Arc::new(token);
        *cached = Some(Arc::clone(&token));
        Ok(token)
    }

    pub(crate) async fn page(&self, path: &str) -> Result<ResourcePage> {
        let mut page: ResourcePage = self.fetch(path).await?;
        let url = Self::url(path)?;
        if url.path().starts_with("/v1/catalog/") && url.path().ends_with("/tracks") {
            self.hydrate_catalog(&mut page.data, "songs", "artists,albums")
                .await?;
        }
        Ok(page)
    }

    /// Search pages use results[type]. Apple's 25-item cap must not skip entries when
    /// Mineral requests a larger page: follow next until the requested prefix is filled.
    pub(crate) async fn search(
        &self,
        query: &str,
        collection: &str,
        page: Page,
    ) -> Result<PageResult<Resource>> {
        let session = self.session().await?;
        let mut url = Url::parse(&format!(
            "https://amp-api.music.apple.com/v1/catalog/{}/search",
            session.storefront
        ))?;
        url.query_pairs_mut()
            .append_pair("term", query)
            .append_pair("types", collection)
            .append_pair("offset", &page.offset.to_string());
        let mut next = Some(url.to_string());
        let mut resources = Vec::new();
        while u32::try_from(resources.len())? < page.limit {
            let Some(path) = next.take() else {
                break;
            };
            let mut url = Self::url(&path)?;
            let pairs: Vec<(String, String)> = url
                .query_pairs()
                .filter(|(key, _)| key != "limit")
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
            let remaining = page.limit - u32::try_from(resources.len())?;
            url.query_pairs_mut()
                .clear()
                .extend_pairs(pairs)
                .append_pair("limit", &remaining.min(25).to_string());
            let mut response: SearchResponse = self.fetch(url.as_str()).await?;
            // An absent result group represents no matches for that requested type.
            let Some(result) = response.results.remove(collection) else {
                break;
            };
            resources.extend(result.data);
            next = result.next;
        }
        if matches!(collection, "songs" | "albums") {
            let include = if collection == "songs" {
                "artists,albums"
            } else {
                "artists"
            };
            self.hydrate_catalog(&mut resources, collection, include)
                .await?;
        }
        Ok(PageResult::new(resources, next.is_some()))
    }

    /// Search results and track relationships omit expanded artist/album metadata.
    /// Batch catalog lookups add those relationships without changing membership order.
    async fn hydrate_catalog(
        &self,
        resources: &mut [Resource],
        collection: &str,
        include: &str,
    ) -> Result<()> {
        let session = self.session().await?;
        let ids: Vec<&str> = resources
            .iter()
            .filter(|resource| resource.kind == collection)
            .map(|resource| resource.id.as_str())
            .collect();
        let mut details = FxHashMap::default();
        for batch in ids.chunks(100) {
            let mut url = Url::parse(&format!(
                "https://amp-api.music.apple.com/v1/catalog/{}/{collection}",
                session.storefront
            ))?;
            url.query_pairs_mut()
                .append_pair("ids", &batch.join(","))
                .append_pair("include", include);
            let page: ResourcePage = self.fetch(url.as_str()).await?;
            for resource in page.data {
                details.insert(resource.id.clone(), resource);
            }
        }
        for resource in resources {
            if let Some(detail) = details.get(&resource.id) {
                *resource = detail.clone();
            }
        }
        Ok(())
    }

    async fn fetch<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.session().await?;
        let url = Self::url(path)?;
        let body = self
            .authenticated_json(|| self.client.get(url.clone()), |_| false)
            .await?;
        mineral_log::debug!(target: "apple_music", path = url.path(), "metadata response");
        Ok(body)
    }

    /// Album responses carry their first track page; follow only its remaining pages.
    pub(crate) async fn album_tracks(&self, album: &mut Resource) -> Result<Vec<Resource>> {
        let page = album
            .relationships
            .tracks
            .take()
            .ok_or(Error::InvalidData {
                field: "album tracks relationship",
            })?;
        let mut tracks = page.data;
        let mut next = page.next;
        while let Some(path) = next {
            let page: ResourcePage = self.fetch(&path).await?;
            tracks.extend(page.data);
            next = page.next;
        }
        if album.kind == "albums" {
            // The owning album supplies each track's album reference.
            self.hydrate_catalog(&mut tracks, "songs", "artists")
                .await?;
        }
        Ok(tracks)
    }

    /// Library IDs address library endpoints, retaining personal album track subsets.
    pub(crate) async fn resource_path(&self, collection: &str, id: &str) -> Result<String> {
        if !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
            || id.is_empty()
        {
            return Err(Error::InvalidData {
                field: "resource identity",
            });
        }
        let library = match collection {
            "songs" => id.starts_with("i."),
            "albums" => id.starts_with("l."),
            "artists" => id.starts_with("r."),
            _ => false,
        };
        let path = if library {
            format!("/v1/me/library/{collection}/{id}")
        } else {
            let session = self.session().await?;
            format!("/v1/catalog/{}/{collection}/{id}", session.storefront)
        };
        Ok(path)
    }

    pub(crate) async fn resource(&self, collection: &str, id: &str) -> Result<Resource> {
        let path = self.resource_path(collection, id).await?;
        self.page(&path)
            .await?
            .data
            .into_iter()
            .next()
            .ok_or(Error::Http { status: 404 })
    }

    /// Validate remote continuation origins before attaching account credentials.
    pub(crate) fn url(path: &str) -> Result<Url> {
        let base = Url::parse("https://amp-api.music.apple.com")?;
        let mut url = base.join(path)?;
        if url.origin() != base.origin() || !url.path().starts_with("/v1/") {
            return Err(Error::InvalidData {
                field: "pagination origin",
            });
        }
        let keys: Vec<String> = url.query_pairs().map(|(key, _)| key.into_owned()).collect();
        let path = url.path().to_owned();
        let library_album_path = path.strip_prefix("/v1/me/library/albums/");
        let library_album_detail = library_album_path.is_some_and(|id| !id.contains('/'));
        let library_album_tracks = library_album_path.is_some_and(|path| {
            path.split_once('/')
                .is_some_and(|(_, relationship)| relationship == "tracks")
        });
        let library_album_request = library_album_detail || library_album_tracks;
        let collection = path
            .rsplit('/')
            .next()
            .is_some_and(|last| matches!(last, "songs" | "albums" | "tracks"));
        let catalog_tracks = path.starts_with("/v1/catalog/") && path.ends_with("/tracks");
        let include = if library_album_detail {
            Some("catalog,tracks")
        } else if path.starts_with("/v1/me/library/") {
            Some("catalog")
        } else if catalog_tracks {
            None
        } else if path.ends_with("/songs") || path.contains("/songs/") {
            Some("artists,albums")
        } else if path.ends_with("/albums") || path.contains("/albums/") {
            Some("artists")
        } else {
            None
        };
        let mut query = url.query_pairs_mut();
        let mut defaults = Vec::new();
        if path.starts_with("/v1/me/library/") {
            // Album navigation supplies the owning album; artist links need only names.
            if library_album_request {
                defaults.push(("include[songs]", "artists"));
                defaults.push(("fields[artists]", "name"));
                // Suppressing automatic relationships requires explicit tracks above.
                defaults.push(("omit[resource]", "autos"));
            } else {
                defaults.push(("include[songs]", "artists,albums"));
            }
            defaults.push(("include[albums]", "artists"));
        }
        if collection && !keys.iter().any(|key| key == "ids") {
            defaults.push(("limit", "100"));
        }
        if let Some(include) = include {
            defaults.push(("include", include));
        }
        for (key, value) in defaults {
            if !keys.iter().any(|existing| existing == key) {
                query.append_pair(key, value);
            }
        }
        drop(query);
        Ok(url)
    }
}
