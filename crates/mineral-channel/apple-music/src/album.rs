//! Loads complete album details from the source-scoped database or Apple.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, Utc};
use mineral_channel_core::store::{AlbumCacheEntry, NamespaceStore};
use mineral_model::{Album, AlbumId, AlbumRef, AlbumTrack, CollectionIndex};
use rustc_hash::FxHasher;

use crate::api::Api;
use crate::config::AlbumCacheConfig;
use crate::{Result, mapping};

pub(crate) struct AlbumLoader {
    persist: Option<Arc<dyn NamespaceStore>>,

    ttl: TimeDelta,

    ttl_jitter: TimeDelta,
}

impl AlbumLoader {
    pub(crate) fn new(config: &AlbumCacheConfig, persist: Option<Arc<dyn NamespaceStore>>) -> Self {
        Self {
            persist,
            ttl: TimeDelta::days(i64::from(*config.ttl_days())),
            ttl_jitter: TimeDelta::days(i64::from(*config.ttl_jitter_days())),
        }
    }

    /// Library and catalog IDs own separate snapshots, preserving saved track subsets.
    pub(crate) async fn load(&self, api: &Api, id: &AlbumId) -> Result<Album> {
        if let Some(store) = &self.persist
            && let Some(entry) = store.get_album_cache(id).await?
            && entry.expired_at > Utc::now()
        {
            mineral_log::debug!(target: "apple_music", album = %id,
                expired_at = %entry.expired_at, "album cache hit");
            return Ok(entry.album);
        }
        let album = fetch(api, id).await?;
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

    /// Choose the jitter
    fn lifetime(&self, id: &AlbumId, loaded_at: DateTime<Utc>) -> Result<TimeDelta> {
        let mut hasher = FxHasher::default();
        (id.namespace().name(), id.value(), loaded_at).hash(&mut hasher);
        let jitter_ms = self.ttl_jitter.num_milliseconds();
        let range = u64::try_from(jitter_ms * 2 + 1)?;
        let offset = i64::try_from(hasher.finish() % range)? - jitter_ms;
        Ok(self.ttl + TimeDelta::milliseconds(offset))
    }
}

async fn fetch(api: &Api, id: &AlbumId) -> Result<Album> {
    let mut resource = api.resource("albums", id.as_str()).await?;
    let mut album = mapping::album(&resource)?;
    let tracks = api.album_tracks(&mut resource).await?;
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
