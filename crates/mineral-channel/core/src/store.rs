//! Scoped and cross-source storage interfaces supplied by the daemon.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use mineral_model::{
    Album, AlbumId, ArtistId, CollectionIndex, Envelope, Playlist, PlaylistId, Song, SongId,
    SourceKind, StoreValue,
};
use rustc_hash::{FxHashMap, FxHashSet};

/// A failed operation through a storage interface.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// An operation used an identifier belonging to another source.
    #[error("expected source {expected:?}, got {actual:?}")]
    NamespaceMismatch {
        /// Source bound to the store.
        expected: SourceKind,

        /// Source carried by the identifier.
        actual: SourceKind,
    },

    /// The storage implementation failed, retaining its structured cause.
    #[error("storage operation failed")]
    Backend {
        /// Original storage error.
        #[source]
        source: crate::error::BoxError,
    },
}

/// A storage interface operation result.
pub type StoreResult<T> = std::result::Result<T, StoreError>;

/// 一条持久化的 Playlist membership relation。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedPlaylistEntry {
    /// Canonical snapshot 中的 0-based absolute coordinate。
    pub index: CollectionIndex,

    /// Relation 指向的 SongId，保留 Song 自己的 namespace。
    pub song_id: SongId,
}

/// 歌单缓存出参(显式 relation 保序，展示时配 song_meta 重建)。
#[derive(Debug, Clone)]
pub struct PlaylistCacheEntry {
    /// 歌单名(可空)。
    pub name: Option<String>,

    /// 抓取时刻 unix ms。
    pub fetched_at: i64,

    /// 歌单版本戳(网易云 `trackUpdateTime`,unix ms;旧库或未知为 `None`)。
    ///
    /// 曲目增删改/重排会更新它,供调用方做条件刷新的版本比对。
    pub track_update_time: Option<i64>,

    /// Playlist membership，按 collection index 升序。
    pub entries: Vec<CachedPlaylistEntry>,
}

/// A complete album detail snapshot; its owner decides when it expires.
#[derive(Debug, Clone)]
pub struct AlbumCacheEntry {
    /// Album metadata and ordered tracks from the same successful fetch.
    pub album: Album,

    /// Expiration time chosen by the channel in UTC; reads do not extend it.
    pub expired_at: DateTime<Utc>,
}

/// Reads and writes one fixed source; implementations reject mismatched owner IDs even when disabled.
///
/// Song projections also require matching album and artist sources. Playlist members may reference
/// other sources, but reading those songs requires their owners’ stores. No connection or re-scoping
/// operation is exposed. Disabled stores return empty reads and discard valid writes.
#[async_trait]
pub trait NamespaceStore: Send + Sync {
    /// Returns the source fixed when this store was created.
    fn source(&self) -> SourceKind;

    /// Reads an album name from this source.
    async fn album_name(&self, id: &AlbumId) -> StoreResult<Option<String>>;

    /// Reads a complete album snapshot and the expiration time chosen by its channel.
    async fn get_album_cache(&self, id: &AlbumId) -> StoreResult<Option<AlbumCacheEntry>>;

    /// Atomically replaces this source's complete album snapshot and expiration time.
    async fn put_album_cache(&self, entry: &AlbumCacheEntry) -> StoreResult<()>;

    /// Reads an artist name from this source.
    async fn artist_name(&self, id: &ArtistId) -> StoreResult<Option<String>>;

    /// Reads one song; missing metadata returns None.
    async fn get_meta(&self, id: &SongId) -> StoreResult<Option<Song>>;

    /// Reads this source’s songs in batches, omitting missing records.
    async fn get_meta_batch(&self, ids: &[SongId]) -> StoreResult<FxHashMap<SongId, Song>>;

    /// Lists this source’s stored songs.
    async fn list_meta(&self) -> StoreResult<Vec<Song>>;

    /// Merges supplied fields into one song; absent fields retain stored values.
    async fn upsert_meta(&self, song: &Song) -> StoreResult<()>;

    /// Validates all song identities before merging the batch atomically.
    async fn upsert_meta_batch(&self, songs: &[&Song]) -> StoreResult<()>;

    /// Validates all identities before replacing this source’s complete song projection.
    async fn replace_meta_batch(&self, songs: &[&Song]) -> StoreResult<()>;

    /// Changes favorite membership; returns whether membership changed.
    async fn set_loved(&self, id: &SongId, loved: bool) -> StoreResult<bool>;

    /// Reads this source’s favorite membership.
    async fn is_loved(&self, id: &SongId) -> StoreResult<bool>;

    /// Lists this source’s favorite identities.
    async fn loved_ids(&self) -> StoreResult<FxHashSet<SongId>>;

    /// Replaces a playlist owned by this source; member references retain their own source identities.
    async fn put_playlist_cache(
        &self,
        id: &PlaylistId,
        name: Option<&str>,
        track_update_time: Option<i64>,
        entries: &[CachedPlaylistEntry],
    ) -> StoreResult<()>;

    /// Reads this source’s playlist membership in collection order.
    async fn get_playlist_cache(&self, id: &PlaylistId) -> StoreResult<Option<PlaylistCacheEntry>>;

    /// Reads a song value; missing keys return Nil.
    async fn kv_get(&self, id: &SongId, key: &str) -> StoreResult<StoreValue>;

    /// Writes a song value; Nil removes the key and reserved keys are rejected.
    async fn kv_set(&self, id: &SongId, key: &str, value: &StoreValue) -> StoreResult<()>;

    /// Writes a rating from zero to five; None clears it.
    async fn set_rating(&self, id: &SongId, rating: Option<u8>) -> StoreResult<()>;

    /// Reads a song rating; unrated songs return None.
    async fn query_rating(&self, id: &SongId) -> StoreResult<Option<u8>>;

    /// Replaces this song’s amplitude envelope.
    async fn put_envelope(&self, id: &SongId, envelope: &Envelope) -> StoreResult<()>;

    /// Reads an envelope matching the algorithm version; other versions return None.
    async fn get_envelope(&self, id: &SongId, version: u16) -> StoreResult<Option<Envelope>>;
}

/// Provides library operations across sources.
#[async_trait]
pub trait LibraryStore: Send + Sync {
    /// Lists user-created playlist headers without loading songs.
    async fn user_playlists(&self) -> StoreResult<Vec<Playlist>>;

    /// Reads a user-created playlist with its ordered song snapshots.
    async fn user_playlist(&self, id: &PlaylistId) -> StoreResult<Option<Playlist>>;

    /// Lists favorites with available song metadata, newest first, retaining each song’s source.
    async fn loved_songs(&self) -> StoreResult<Vec<Song>>;

    /// Counts the same metadata-backed favorites returned by `loved_songs`.
    async fn loved_count(&self) -> StoreResult<u64>;
}
