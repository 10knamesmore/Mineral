//! Catalog artist details and a separate, metadata-only album listing.

use async_stream::stream;
use mineral_channel_core::{Batch, BatchStream, Page};
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

    /// 保留 continuation，映射成功后才推进；专辑曲目另走 album_detail。
    pub(crate) fn artist_albums<'a>(&'a self, id: &'a ArtistId) -> BatchStream<'a, Album> {
        Box::pin(stream! {
            let catalog_id = loop {
                match self.artist_catalog_id(id).await {
                    Ok(id) => break id,
                    Err(error) => yield Err(error.into()),
                }
            };
            let mut cursor = PageCursor::First;
            loop {
                let result: Result<_> = async {
                    let request = match &cursor {
                        PageCursor::Next(continuation) => Request::Next {
                            continuation,
                            kind: PageKind::ArtistAlbums,
                            limit: None,
                        },
                        PageCursor::First => Request::ArtistAlbums {
                            id: &catalog_id,
                            page: Page::default(),
                        },
                    };
                    let response = self.page(request).await?;
                    let items = response.data.iter().map(mapping::album)
                        .collect::<Result<Vec<_>>>()?;
                    Ok((items, response.next))
                }.await;
                match result {
                    Ok((items, next)) => {
                        yield Ok(Batch { items, complete: next.is_none() });
                        let Some(next) = next else { return };
                        cursor = PageCursor::Next(next);
                    }
                    Err(error) => yield Err(error.into()),
                }
            }
        })
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
