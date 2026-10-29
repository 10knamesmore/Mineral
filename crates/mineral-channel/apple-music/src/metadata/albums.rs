//! Loads complete album details from the source-scoped database or Apple.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use mineral_channel_core::store::{AlbumCacheEntry, NamespaceStore};
use mineral_model::{Album, AlbumId, AlbumRef, AlbumTrack, CollectionIndex};
use rustc_hash::FxHasher;

use super::{MetadataClient, mapping};
use crate::Result;
use crate::config::AlbumCacheConfig;

/// Album-detail cache policy for one channel; entries live in its optional namespace store.
/// A cache hit returns the saved snapshot without extending its expiration.
pub(super) struct AlbumLoader {
    persist: Option<Arc<dyn NamespaceStore>>,

    ttl: TimeDelta,

    ttl_jitter: TimeDelta,
}

impl AlbumLoader {
    pub(super) fn new(config: &AlbumCacheConfig, persist: Option<Arc<dyn NamespaceStore>>) -> Self {
        Self {
            persist,
            ttl: TimeDelta::days(i64::from(*config.ttl_days())),
            ttl_jitter: TimeDelta::days(i64::from(*config.ttl_jitter_days())),
        }
    }

    /// Library and catalog IDs own separate snapshots, preserving saved track subsets.
    async fn load(&self, metadata: &MetadataClient, id: &AlbumId) -> Result<Album> {
        if let Some(store) = &self.persist
            && let Some(entry) = store.get_album_cache(id).await?
            && entry.expired_at > Utc::now()
        {
            mineral_log::debug!(target: "apple_music", album = %id,
                expired_at = %entry.expired_at, "album cache hit");
            return Ok(entry.album);
        }
        let album = fetch_album(metadata, id).await?;
        let loaded_at = Utc::now();
        let entry = AlbumCacheEntry {
            album,
            expired_at: loaded_at + self.lifetime(id, loaded_at)?,
        };
        if let Some(store) = &self.persist {
            store.put_album_cache(&entry).await?;
        }
        mineral_log::debug!(target: "apple_music", album = %id,
            tracks = entry.album.tracks.len(), expired_at = %entry.expired_at,
            "album detail loaded");
        Ok(entry.album)
    }

    /// Spread expirations within ±ttl_jitter using the album identity and fetch time.
    fn lifetime(&self, id: &AlbumId, loaded_at: DateTime<Utc>) -> Result<TimeDelta> {
        let mut hasher = FxHasher::default();
        (id.namespace().name(), id.value(), loaded_at).hash(&mut hasher);
        let jitter_ms = self.ttl_jitter.num_milliseconds();
        let range = u64::try_from(jitter_ms * 2 + 1)?;
        let offset = i64::try_from(hasher.finish() % range)? - jitter_ms;
        Ok(self.ttl + TimeDelta::milliseconds(offset))
    }
}

impl MetadataClient {
    /// Return a complete album snapshot, loading missing track pages on a cache miss.
    pub(crate) async fn album_detail(&self, id: &AlbumId) -> Result<Album> {
        self.albums.load(self, id).await
    }
}

async fn fetch_album(metadata: &MetadataClient, id: &AlbumId) -> Result<Album> {
    let mut resource = metadata.resource("albums", id.as_str()).await?;
    let mut album = mapping::album(&resource)?;
    let tracks = metadata.album_tracks(&mut resource).await?;
    for (resource, index) in tracks.iter().zip(0_u64..) {
        if !matches!(resource.kind.as_str(), "songs" | "library-songs") {
            continue;
        }
        let mut song = mapping::song(resource)?;
        // Album navigation retains this collection, including a saved track subset.
        song.album = Some(AlbumRef {
            id: album.id.clone(),
            name: album.name.clone(),
        });
        album.tracks.push(
            AlbumTrack::builder()
                .index(CollectionIndex::new(index))
                .song(song)
                .build(),
        );
    }
    Ok(album)
}
