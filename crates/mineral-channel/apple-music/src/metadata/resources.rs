//! Catalog/library addressing, continuation pages and relationship expansion.
//! URL defaults preserve library album membership and avoid redundant track lookups.

use mineral_channel_core::{Page, PageResult};
use rustc_hash::FxHashMap;
use serde::de::DeserializeOwned;
use url::Url;

use super::MetadataClient;
use super::wire::{Resource, ResourcePage, SearchResponse};
use crate::{Error, Result};

impl MetadataClient {
    pub(super) async fn page(&self, path: &str) -> Result<ResourcePage> {
        let mut page: ResourcePage = self.fetch(path).await?;
        let url = Self::resource_url(path)?;
        if url.path().starts_with("/v1/catalog/") && url.path().ends_with("/tracks") {
            self.hydrate_catalog(&mut page.data, "songs", "artists,albums")
                .await?;
        }
        Ok(page)
    }

    /// Search pages use results[type]. Apple's 25-item cap must not skip entries when
    /// Mineral requests a larger page: follow next until the requested prefix is filled.
    pub(super) async fn search_resources(
        &self,
        query: &str,
        collection: &str,
        page: Page,
    ) -> Result<PageResult<Resource>> {
        let account = self.account.account().await?;
        let mut url = Url::parse(&format!(
            "https://amp-api.music.apple.com/v1/catalog/{}/search",
            account.storefront
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
            let mut url = Self::resource_url(&path)?;
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
        let account = self.account.account().await?;
        let ids: Vec<&str> = resources
            .iter()
            .filter(|resource| resource.kind == collection)
            .map(|resource| resource.id.as_str())
            .collect();
        let mut details = FxHashMap::default();
        for batch in ids.chunks(100) {
            let mut url = Url::parse(&format!(
                "https://amp-api.music.apple.com/v1/catalog/{}/{collection}",
                account.storefront
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
        self.account.account().await?;
        let url = Self::resource_url(path)?;
        let body = self
            .account
            .authenticated_json(|| self.account.http().get(url.clone()), |_| false)
            .await?;
        mineral_log::debug!(target: "apple_music", path = url.path(), "metadata response");
        Ok(body)
    }

    /// Album responses carry their first track page; follow only its remaining pages.
    pub(super) async fn album_tracks(&self, album: &mut Resource) -> Result<Vec<Resource>> {
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

    /// Library IDs address library endpoints, retaining personal collection membership.
    pub(super) async fn resource_path(&self, collection: &str, id: &str) -> Result<String> {
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
            "playlists" => id.starts_with("p."),
            _ => false,
        };
        let path = if library {
            format!("/v1/me/library/{collection}/{id}")
        } else {
            let account = self.account.account().await?;
            format!("/v1/catalog/{}/{collection}/{id}", account.storefront)
        };
        Ok(path)
    }

    pub(super) async fn resource(&self, collection: &str, id: &str) -> Result<Resource> {
        let path = self.resource_path(collection, id).await?;
        self.page(&path)
            .await?
            .data
            .into_iter()
            .next()
            .ok_or(Error::Http { status: 404 })
    }

    /// Resolve paths and continuation URLs with resource-specific query defaults.
    /// Only AMP /v1 URLs can receive account credentials through metadata requests.
    pub(super) fn resource_url(path: &str) -> Result<Url> {
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
        let library_playlist_list = path == "/v1/me/library/playlists";
        let library_playlist_detail = path
            .strip_prefix("/v1/me/library/playlists/")
            .is_some_and(|id| !id.contains('/'));
        let collection = path
            .rsplit('/')
            .next()
            .is_some_and(|last| matches!(last, "songs" | "albums" | "tracks" | "playlists"));
        let catalog_tracks = path.starts_with("/v1/catalog/") && path.ends_with("/tracks");
        let include = if library_album_detail {
            Some("catalog,tracks")
        } else if library_playlist_list || library_playlist_detail {
            Some("tracks")
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
        if library_playlist_list {
            // A single relationship item supplies meta.total without loading each playlist.
            defaults.push(("limit[tracks]", "1"));
        } else if library_playlist_detail {
            defaults.push(("limit[tracks]", "100"));
            defaults.push(("include[library-songs]", "catalog"));
        }
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
