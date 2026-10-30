//! Constructs a channel and adapts metadata and playback operations to Mineral's traits.

use std::sync::Arc;

use async_trait::async_trait;
use mineral_channel_core::store::NamespaceStore;
use mineral_channel_core::{
    ArtistSectionKind, ArtistSections, ChannelCaps, MusicChannel, Page, PageResult, PlaylistDetail,
    PlaylistLoad,
};
use mineral_model::CachePolicy;
use mineral_model::{
    Album, AlbumId, Artist, ArtistId, Lyrics, Playlist, PlaylistId, SearchKind, Song, SongId,
    SourceKind,
};
use mineral_playback::{PlaybackProvider, PlaybackRequest, PreparedPlayback};
use tokio_util::sync::CancellationToken;

use crate::account::AccountClient;
use crate::metadata::MetadataClient;
use crate::playback;
use crate::{AppleMusicConfig, Result, StoredAppleMusicAuth};

/// An authenticated, read-only source sharing its account context with playback.
pub struct AppleMusicChannel {
    account: Arc<AccountClient>,

    metadata: MetadataClient,

    config: AppleMusicConfig,
}

impl AppleMusicChannel {
    /// Construction is local; the first request verifies the saved credential.
    pub fn with_credential(
        config: &AppleMusicConfig,
        auth: StoredAppleMusicAuth,
        persist: Option<Arc<dyn NamespaceStore>>,
    ) -> Result<Self> {
        let account = AccountClient::new(config, auth)?;
        Ok(Self {
            metadata: MetadataClient::new(account.clone(), config, persist),
            account,
            config: config.clone(),
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
                SearchKind::Playlist,
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
        Ok(self.metadata.search_songs(query, page).await?)
    }

    async fn search_albums(
        &self,
        query: &str,
        page: Page,
    ) -> mineral_channel_core::Result<PageResult<Album>> {
        Ok(self.metadata.search_albums(query, page).await?)
    }

    async fn search_artists(
        &self,
        query: &str,
        page: Page,
    ) -> mineral_channel_core::Result<PageResult<Artist>> {
        Ok(self.metadata.search_artists(query, page).await?)
    }

    async fn search_playlists(
        &self,
        query: &str,
        page: Page,
    ) -> mineral_channel_core::Result<PageResult<Playlist>> {
        Ok(self.metadata.search_playlists(query, page).await?)
    }

    async fn songs_detail(
        &self,
        ids: &[SongId],
        _cache: CachePolicy,
    ) -> mineral_channel_core::Result<Vec<Song>> {
        Ok(self.metadata.songs_detail(ids).await?)
    }

    async fn lyrics(&self, id: &SongId) -> mineral_channel_core::Result<Lyrics> {
        Ok(self.metadata.lyrics(id).await?)
    }

    async fn my_albums(&self, _cache: CachePolicy) -> mineral_channel_core::Result<Vec<Album>> {
        Ok(self.metadata.my_albums().await?)
    }

    async fn my_playlists(
        &self,
        _cache: CachePolicy,
    ) -> mineral_channel_core::Result<Vec<Playlist>> {
        Ok(self.metadata.my_playlists().await?)
    }

    async fn playlist_detail(
        &self,
        id: &PlaylistId,
        load: PlaylistLoad,
        _cache: CachePolicy,
    ) -> mineral_channel_core::Result<PlaylistDetail> {
        Ok(self.metadata.playlist_detail(id, load).await?)
    }

    async fn album_detail(
        &self,
        id: &AlbumId,
        cache: CachePolicy,
    ) -> mineral_channel_core::Result<Album> {
        Ok(self.metadata.album_detail(id, cache).await?)
    }

    async fn artist_detail(&self, id: &ArtistId) -> mineral_channel_core::Result<Artist> {
        Ok(self.metadata.artist_detail(id).await?)
    }

    async fn artist_albums(
        &self,
        id: &ArtistId,
        page: Page,
    ) -> mineral_channel_core::Result<PageResult<Album>> {
        Ok(self.metadata.artist_albums(id, page).await?)
    }
}

#[async_trait]
impl PlaybackProvider for AppleMusicChannel {
    fn source(&self) -> SourceKind {
        SourceKind::APPLE_MUSIC
    }

    async fn resolve(
        &self,
        request: PlaybackRequest,
        cancellation: CancellationToken,
    ) -> mineral_playback::Result<Box<dyn PreparedPlayback>> {
        playback::prepare(
            &self.metadata,
            self.account.clone(),
            &self.config,
            request,
            cancellation,
        )
        .await
    }
}
