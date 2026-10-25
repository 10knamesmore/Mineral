//! Read-only catalog search, metadata and personal library collections.

use std::sync::Arc;

use async_trait::async_trait;
use mineral_channel_core::store::NamespaceStore;
use mineral_channel_core::{
    ArtistSectionKind, ArtistSections, ChannelCaps, MusicChannel, Page, PageResult,
};
use mineral_model::{
    Album, AlbumId, Artist, ArtistId, Lyrics, SearchKind, Song, SongId, SourceKind,
};

use crate::album::AlbumLoader;
use crate::api::Api;
use crate::artist;
use crate::lyrics;
use crate::mapping;
use crate::{AppleMusicConfig, Result, StoredAppleMusicAuth};

/// An authenticated, read-only source sharing its account context with playback.
pub struct AppleMusicChannel {
    pub(crate) api: Arc<Api>,

    albums: AlbumLoader,
}

impl AppleMusicChannel {
    /// Construction is local; the first request verifies the saved credential.
    pub fn with_credential(
        config: &AppleMusicConfig,
        auth: StoredAppleMusicAuth,
        persist: Option<Arc<dyn NamespaceStore>>,
    ) -> Result<Self> {
        let api = Api::new(config, auth)?;
        Ok(Self {
            albums: AlbumLoader::new(config.album_cache(), persist),
            api,
        })
    }
}

#[async_trait]
impl MusicChannel for AppleMusicChannel {
    fn source(&self) -> SourceKind {
        SourceKind::APPLE_MUSIC
    }

    fn caps(&self) -> ChannelCaps {
        ChannelCaps::builder()
            .searchable(vec![
                SearchKind::Song,
                SearchKind::Album,
                SearchKind::Artist,
            ])
            .playlist_edit(false)
            .artist_sections(ArtistSections::new(vec![
                ArtistSectionKind::TopSongs,
                ArtistSectionKind::Albums,
            ]))
            .build()
    }

    async fn search_songs(
        &self,
        query: &str,
        page: Page,
    ) -> mineral_channel_core::Result<PageResult<Song>> {
        let result = self.api.search(query, "songs", page).await?;
        Ok(PageResult {
            items: result
                .items
                .iter()
                .map(mapping::song)
                .collect::<Result<_>>()?,
            has_more: result.has_more,
        })
    }

    async fn search_albums(
        &self,
        query: &str,
        page: Page,
    ) -> mineral_channel_core::Result<PageResult<Album>> {
        let result = self.api.search(query, "albums", page).await?;
        Ok(PageResult {
            items: result
                .items
                .iter()
                .map(mapping::album)
                .collect::<Result<_>>()?,
            has_more: result.has_more,
        })
    }

    async fn search_artists(
        &self,
        query: &str,
        page: Page,
    ) -> mineral_channel_core::Result<PageResult<Artist>> {
        let result = self.api.search(query, "artists", page).await?;
        Ok(PageResult {
            items: result
                .items
                .iter()
                .map(mapping::artist)
                .collect::<Result<_>>()?,
            has_more: result.has_more,
        })
    }

    async fn songs_detail(&self, ids: &[SongId]) -> mineral_channel_core::Result<Vec<Song>> {
        let mut songs = Vec::with_capacity(ids.len());
        for id in ids {
            let resource = self.api.resource("songs", id.as_str()).await?;
            let mut song = mapping::song(&resource)?;
            song.id = id.clone();
            songs.push(song);
        }
        Ok(songs)
    }

    async fn lyrics(&self, id: &SongId) -> mineral_channel_core::Result<Lyrics> {
        Ok(lyrics::load(&self.api, id.as_str()).await?)
    }

    async fn my_albums(&self) -> mineral_channel_core::Result<Vec<Album>> {
        let mut path = "/v1/me/library/albums".to_owned();
        let mut albums = Vec::new();
        loop {
            let page = self.api.page(&path).await?;
            albums.extend(
                page.data
                    .iter()
                    .map(mapping::album)
                    .collect::<Result<Vec<_>>>()?,
            );
            let Some(next) = page.next else {
                break;
            };
            path = next;
        }
        mineral_log::info!(target: "apple_music", albums = albums.len(), "saved albums loaded");
        Ok(albums)
    }

    async fn album_detail(&self, id: &AlbumId) -> mineral_channel_core::Result<Album> {
        Ok(self.albums.load(&self.api, id).await?)
    }

    async fn artist_detail(&self, id: &ArtistId) -> mineral_channel_core::Result<Artist> {
        Ok(artist::detail(&self.api, id).await?)
    }

    async fn artist_albums(
        &self,
        id: &ArtistId,
        page: Page,
    ) -> mineral_channel_core::Result<PageResult<Album>> {
        Ok(artist::albums(&self.api, id, page).await?)
    }
}
