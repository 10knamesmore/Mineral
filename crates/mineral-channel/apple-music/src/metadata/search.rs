//! Search responses retain Apple's membership order and expose shared model types.

use mineral_channel_core::{Page, PageResult};
use mineral_model::{Album, Artist, Playlist, Song};

use super::request::SongRelationships;
use super::{MetadataClient, mapping};
use crate::Result;

impl MetadataClient {
    pub(crate) async fn search_playlists(
        &self,
        query: &str,
        page: Page,
    ) -> Result<PageResult<Playlist>> {
        let result = self.search_resources(query, page).await?;
        Ok(PageResult {
            items: result
                .items
                .iter()
                .map(mapping::playlist)
                .collect::<Result<_>>()?,
            has_more: result.has_more,
        })
    }

    pub(crate) async fn search_songs(&self, query: &str, page: Page) -> Result<PageResult<Song>> {
        let mut result = self.search_resources(query, page).await?;
        self.hydrate_songs(&mut result.items, SongRelationships::ArtistsAndAlbums)
            .await?;
        Ok(PageResult {
            items: result.items.iter().map(mapping::song).collect(),
            has_more: result.has_more,
        })
    }

    pub(crate) async fn search_albums(&self, query: &str, page: Page) -> Result<PageResult<Album>> {
        let mut result = self.search_resources(query, page).await?;
        self.hydrate_albums(&mut result.items).await?;
        Ok(PageResult {
            items: result
                .items
                .iter()
                .map(mapping::album)
                .collect::<Result<_>>()?,
            has_more: result.has_more,
        })
    }

    pub(crate) async fn search_artists(
        &self,
        query: &str,
        page: Page,
    ) -> Result<PageResult<Artist>> {
        let result = self.search_resources(query, page).await?;
        Ok(PageResult {
            items: result
                .items
                .iter()
                .map(mapping::artist)
                .collect::<Result<_>>()?,
            has_more: result.has_more,
        })
    }
}
