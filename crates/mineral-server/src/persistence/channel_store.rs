//! Adapts the daemon’s SQLite store to the channel storage interface.

use super::{Error, NamespaceStore, ServerStore};
use mineral_channel_core::store::{
    CachedPlaylistEntry, LibraryStore, NamespaceStore as ChannelStore, PlaylistCacheEntry,
    StoreError, StoreResult,
};
use mineral_model::{
    AlbumId, ArtistId, Envelope, Playlist, PlaylistId, Song, SongId, SourceKind, StoreValue,
};
use rustc_hash::{FxHashMap, FxHashSet};

impl From<Error> for StoreError {
    fn from(error: Error) -> Self {
        match error {
            Error::NamespaceMismatch { expected, actual } => {
                Self::NamespaceMismatch { expected, actual }
            }
            source => Self::Backend {
                source: Box::new(source),
            },
        }
    }
}

#[async_trait::async_trait]
impl ChannelStore for NamespaceStore {
    fn source(&self) -> SourceKind {
        self.source()
    }

    async fn album_name(&self, id: &AlbumId) -> StoreResult<Option<String>> {
        Self::album_name(self, id).await.map_err(Into::into)
    }

    async fn artist_name(&self, id: &ArtistId) -> StoreResult<Option<String>> {
        Self::artist_name(self, id).await.map_err(Into::into)
    }

    async fn get_meta(&self, id: &SongId) -> StoreResult<Option<Song>> {
        Self::get_meta(self, id).await.map_err(Into::into)
    }

    async fn get_meta_batch(&self, ids: &[SongId]) -> StoreResult<FxHashMap<SongId, Song>> {
        Self::get_meta_batch(self, ids).await.map_err(Into::into)
    }

    async fn list_meta(&self) -> StoreResult<Vec<Song>> {
        Self::list_meta(self).await.map_err(Into::into)
    }

    async fn upsert_meta(&self, song: &Song) -> StoreResult<()> {
        Self::upsert_meta(self, song).await.map_err(Into::into)
    }

    async fn upsert_meta_batch(&self, songs: &[&Song]) -> StoreResult<()> {
        Self::upsert_meta_batch(self, songs)
            .await
            .map_err(Into::into)
    }

    async fn replace_meta_batch(&self, songs: &[&Song]) -> StoreResult<()> {
        Self::replace_meta_batch(self, songs)
            .await
            .map_err(Into::into)
    }

    async fn set_loved(&self, id: &SongId, loved: bool) -> StoreResult<bool> {
        Self::set_loved(self, id, loved).await.map_err(Into::into)
    }

    async fn is_loved(&self, id: &SongId) -> StoreResult<bool> {
        Self::is_loved(self, id).await.map_err(Into::into)
    }

    async fn loved_ids(&self) -> StoreResult<FxHashSet<SongId>> {
        Self::loved_ids(self).await.map_err(Into::into)
    }

    async fn put_playlist_cache(
        &self,
        id: &PlaylistId,
        name: Option<&str>,
        track_update_time: Option<i64>,
        entries: &[CachedPlaylistEntry],
    ) -> StoreResult<()> {
        Self::put_playlist_cache(self, id, name, track_update_time, entries)
            .await
            .map_err(Into::into)
    }

    async fn get_playlist_cache(&self, id: &PlaylistId) -> StoreResult<Option<PlaylistCacheEntry>> {
        Self::get_playlist_cache(self, id).await.map_err(Into::into)
    }

    async fn kv_get(&self, id: &SongId, key: &str) -> StoreResult<StoreValue> {
        Self::kv_get(self, id, key).await.map_err(Into::into)
    }

    async fn kv_set(&self, id: &SongId, key: &str, value: &StoreValue) -> StoreResult<()> {
        Self::kv_set(self, id, key, value).await.map_err(Into::into)
    }

    async fn kv_inc(&self, id: &SongId, key: &str, delta: i64) -> StoreResult<StoreValue> {
        Self::kv_inc(self, id, key, delta).await.map_err(Into::into)
    }

    async fn set_rating(&self, id: &SongId, rating: Option<u8>) -> StoreResult<()> {
        Self::set_rating(self, id, rating).await.map_err(Into::into)
    }

    async fn query_rating(&self, id: &SongId) -> StoreResult<Option<u8>> {
        Self::query_rating(self, id).await.map_err(Into::into)
    }

    async fn put_envelope(&self, id: &SongId, envelope: &Envelope) -> StoreResult<()> {
        Self::put_envelope(self, id, envelope)
            .await
            .map_err(Into::into)
    }

    async fn get_envelope(&self, id: &SongId, version: u16) -> StoreResult<Option<Envelope>> {
        Self::get_envelope(self, id, version)
            .await
            .map_err(Into::into)
    }
}

#[async_trait::async_trait]
impl LibraryStore for ServerStore {
    async fn user_playlists(&self) -> StoreResult<Vec<Playlist>> {
        Self::user_playlists(self).await.map_err(Into::into)
    }

    async fn user_playlist(&self, id: &PlaylistId) -> StoreResult<Option<Playlist>> {
        Self::user_playlist(self, id).await.map_err(Into::into)
    }

    async fn loved_songs(&self) -> StoreResult<Vec<Song>> {
        Self::loved_songs(self).await.map_err(Into::into)
    }

    async fn loved_count(&self) -> StoreResult<u64> {
        Self::loved_count(self).await.map_err(Into::into)
    }
}
