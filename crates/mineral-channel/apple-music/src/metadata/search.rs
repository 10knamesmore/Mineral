//! Search responses retain Apple's membership order and expose shared model types.

use mineral_channel_core::{Page, PageResult};
use mineral_model::{Album, Artist, Playlist, Song};

use super::{MetadataClient, mapping};
use crate::Result;

impl MetadataClient {
    pub(crate) async fn search_playlists(
        &self,
        query: &str,
        page: Page,
    ) -> Result<PageResult<Playlist>> {
        let result = self.search_resources(query, "playlists", page).await?;
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
        let result = self.search_resources(query, "songs", page).await?;
        Ok(PageResult {
            items: result
                .items
                .iter()
                .map(mapping::song)
                .collect::<Result<_>>()?,
            has_more: result.has_more,
        })
    }

    pub(crate) async fn search_albums(&self, query: &str, page: Page) -> Result<PageResult<Album>> {
        let result = self.search_resources(query, "albums", page).await?;
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
        let result = self.search_resources(query, "artists", page).await?;
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
