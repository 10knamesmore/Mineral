//! Executes explicit metadata operations and batches missing catalog relationships.

use mineral_channel_core::{Page, PageResult};
use rustc_hash::FxHashMap;
use serde::de::DeserializeOwned;

use super::MetadataClient;
use super::data::{AlbumResource, PageCursor, ResourceId, ResourcePage, SongResource};
use super::request::{PageKind, Request, SongRelationships};
use super::wire::{DecodeResource, MetadataResource, PageResponse, SearchResponse};
use crate::{Error, Result};

impl MetadataClient {
    pub(super) async fn page<T: DecodeResource>(
        &self,
        request: Request<'_>,
    ) -> Result<ResourcePage<T>> {
        self.fetch::<PageResponse>(request).await?.decode()
    }

    pub(super) async fn resource<T: MetadataResource>(&self, id: &ResourceId) -> Result<T> {
        self.page(Request::Resource { kind: T::KIND, id })
            .await?
            .data
            .into_iter()
            .next()
            .ok_or(Error::Http { status: 404 })
    }

    /// Apple's search cap is 25; follow page boundaries until the requested prefix is filled.
    pub(super) async fn search_resources<T: MetadataResource>(
        &self,
        query: &str,
        page: Page,
    ) -> Result<PageResult<T>> {
        let mut next = Some(PageCursor::First);
        let mut resources = Vec::new();
        while u32::try_from(resources.len())? < page.limit {
            let Some(cursor) = next.take() else {
                break;
            };
            let request = match &cursor {
                PageCursor::Next(continuation) => Request::Next {
                    continuation,
                    kind: PageKind::Search(T::KIND),
                    limit: Some((page.limit - u32::try_from(resources.len())?).min(25)),
                },
                PageCursor::First => Request::Search {
                    kind: T::KIND,
                    query,
                    page,
                },
            };
            let mut response: SearchResponse = self.fetch(request).await?;
            let Some(result) = response.take::<T>()? else {
                break;
            };
            resources.extend(result.data);
            next = result.next.map(PageCursor::Next);
        }
        Ok(PageResult::new(resources, next.is_some()))
    }

    /// Enrich catalog recordings without replacing library membership or reordering duplicates.
    pub(super) async fn hydrate_songs<'a>(
        &self,
        songs: impl IntoIterator<Item = &'a mut SongResource>,
        relationships: SongRelationships,
    ) -> Result<()> {
        let songs: Vec<_> = songs.into_iter().collect();
        let ids: Vec<_> = songs
            .iter()
            .filter_map(|song| match &song.id {
                ResourceId::Catalog(id) => Some(id.as_str()),
                ResourceId::Library(_) => None,
            })
            .collect();
        let mut details = FxHashMap::default();
        for batch in ids.chunks(100) {
            let page: ResourcePage<SongResource> = self
                .page(Request::CatalogSongs {
                    ids: batch,
                    relationships,
                })
                .await?;
            for song in page.data {
                details.insert(song.id.as_str().to_owned(), song);
            }
        }
        for song in songs {
            if let ResourceId::Catalog(id) = &song.id
                && let Some(detail) = details.get(id)
            {
                *song = detail.clone();
            }
        }
        Ok(())
    }

    pub(super) async fn hydrate_albums(&self, albums: &mut [AlbumResource]) -> Result<()> {
        let ids: Vec<_> = albums
            .iter()
            .filter_map(|album| match &album.id {
                ResourceId::Catalog(id) => Some(id.as_str()),
                ResourceId::Library(_) => None,
            })
            .collect();
        let mut details = FxHashMap::default();
        for batch in ids.chunks(100) {
            let page: ResourcePage<AlbumResource> =
                self.page(Request::CatalogAlbums { ids: batch }).await?;
            for album in page.data {
                details.insert(album.id.as_str().to_owned(), album);
            }
        }
        for album in albums {
            if let ResourceId::Catalog(id) = &album.id
                && let Some(detail) = details.get(id)
            {
                *album = detail.clone();
            }
        }
        Ok(())
    }

    pub(super) async fn fetch<T: DeserializeOwned>(&self, request: Request<'_>) -> Result<T> {
        let account = self.account.account().await?;
        let url = request.url(&account.storefront)?;
        let body = self
            .account
            .authenticated_json(|| self.account.http().get(url.clone()), |_| false)
            .await?;
        mineral_log::debug!(target: "apple_music", path = url.path(), "metadata response");
        Ok(body)
    }
}
