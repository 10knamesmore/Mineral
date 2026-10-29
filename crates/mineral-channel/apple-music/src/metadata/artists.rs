//! Catalog artist details and a separate, metadata-only album listing.

use mineral_channel_core::{Page, PageResult};
use mineral_model::{Album, Artist, ArtistId};

use super::data::{ArtistResource, PageCursor, ResourceId, ResourceKind};
use super::request::{PageKind, Request, resource_id};
use super::{MetadataClient, mapping};
use crate::{Error, Result};

impl MetadataClient {
    pub(crate) async fn artist_detail(&self, id: &ArtistId) -> Result<Artist> {
        let catalog_id = self.artist_catalog_id(id).await?;
        let resource: ArtistResource = self
            .page(Request::ArtistDetail { id: &catalog_id })
            .await?
            .data
            .into_iter()
            .next()
            .ok_or(Error::Http { status: 404 })?;
        let mut artist = mapping::artist(&resource)?;
        artist.id = id.clone();
        if let Some(songs) = resource.top_songs {
            artist.songs = songs.data.iter().map(mapping::track).collect();
        }
        Ok(artist)
    }

    /// Album tracks are loaded separately by album_detail.
    pub(crate) async fn artist_albums(
        &self,
        id: &ArtistId,
        page: Page,
    ) -> Result<PageResult<Album>> {
        let catalog_id = self.artist_catalog_id(id).await?;
        let mut next = Some(PageCursor::First);
        let mut albums = Vec::new();
        while u32::try_from(albums.len())? < page.limit {
            let Some(cursor) = next.take() else {
                break;
            };
            let request = match &cursor {
                PageCursor::Next(continuation) => Request::Next {
                    continuation,
                    kind: PageKind::ArtistAlbums,
                    limit: Some((page.limit - u32::try_from(albums.len())?).min(100)),
                },
                PageCursor::First => Request::ArtistAlbums {
                    id: &catalog_id,
                    page,
                },
            };
            let response = self.page(request).await?;
            albums.extend(
                response
                    .data
                    .iter()
                    .map(mapping::album)
                    .collect::<Result<Vec<_>>>()?,
            );
            next = response.next.map(PageCursor::Next);
        }
        Ok(PageResult::new(albums, next.is_some()))
    }

    async fn artist_catalog_id(&self, id: &ArtistId) -> Result<String> {
        let address = resource_id(ResourceKind::Artist, id.as_str())?;
        match address {
            ResourceId::Catalog(id) => Ok(id),
            library @ ResourceId::Library(_) => {
                let resource: ArtistResource = self.resource(&library).await?;
                resource.catalog_id.ok_or(Error::NoCatalogArtist)
            }
        }
    }
}
