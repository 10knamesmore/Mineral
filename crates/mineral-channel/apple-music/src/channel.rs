//! Constructs a channel and adapts metadata and playback operations to Mineral's traits.

use std::sync::Arc;

use async_trait::async_trait;
use mineral_channel_core::collection::{albums_with_cache, playlists_with_cache};
use mineral_channel_core::fetch::fresh_snapshot;
use mineral_channel_core::store::NamespaceStore;
use mineral_channel_core::{
    ArtistSectionKind, ArtistSections, ChannelCaps, CollectionStream, FetchStream, MusicChannel,
    Page, PageResult, PlaylistDetail, PlaylistLoad,
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

    persist: Option<Arc<dyn NamespaceStore>>,
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
            metadata: MetadataClient::new(account.clone(), config, persist.clone()),
            account,
            config: config.clone(),
            persist,
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

    fn my_albums(&self, cache: CachePolicy) -> CollectionStream<'_, Album> {
        albums_with_cache(self.persist.as_deref(), cache, async {
            Ok(self.metadata.my_albums().await?)
        })
    }

    fn my_playlists(&self, cache: CachePolicy) -> CollectionStream<'_, Playlist> {
        playlists_with_cache(self.persist.as_deref(), cache, async {
            Ok(self.metadata.my_playlists().await?)
        })
    }

    fn playlist_detail<'a>(
        &'a self,
        id: &'a PlaylistId,
        load: PlaylistLoad,
        _cache: CachePolicy,
    ) -> FetchStream<'a, PlaylistDetail> {
        fresh_snapshot(async move { Ok(self.metadata.playlist_detail(id, load).await?) })
    }

    fn album_detail<'a>(&'a self, id: &'a AlbumId, cache: CachePolicy) -> FetchStream<'a, Album> {
        self.metadata.album_detail(id, cache)
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
