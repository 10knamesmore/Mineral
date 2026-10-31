//! Loads complete album details from the source-scoped database or Apple.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use async_stream::try_stream;
use chrono::{DateTime, TimeDelta, Utc};
use mineral_channel_core::store::{AlbumCacheEntry, NamespaceStore};
use mineral_channel_core::{FetchSnapshot, FetchStream};
use mineral_model::{Album, AlbumId, AlbumRef, AlbumTrack, CachePolicy, CollectionIndex};
use rustc_hash::FxHasher;

use super::data::{AlbumResource, ResourceKind, ResourceScope, TrackResource};
use super::request::{PageKind, Request, SongRelationships, resource_id};
use super::{MetadataClient, mapping};
use crate::config::AlbumCacheConfig;
use crate::{Error, Result};

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
    fn load<'a>(
        &'a self,
        metadata: &'a MetadataClient,
        id: &'a AlbumId,
        cache: CachePolicy,
    ) -> FetchStream<'a, Album> {
        Box::pin(try_stream! {
            let cached = match (cache, self.persist.as_deref()) {
                (CachePolicy::UseCache, Some(store)) => store.get_album_cache(id).await?,
                _ => None,
            };
            if let Some(entry) = cached {
                mineral_log::debug!(target: "apple_music", album = %id,
                    expired_at = %entry.expired_at, "album cache loaded");
                if entry.expired_at > Utc::now() {
                    yield FetchSnapshot::Fresh(entry.album);
                    return;
                }
                yield FetchSnapshot::Cached(entry.album);
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
            yield FetchSnapshot::Fresh(entry.album);
        })
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
    /// Publish cached details first; expired snapshots are followed by a complete source refresh.
    pub(crate) fn album_detail<'a>(
        &'a self,
        id: &'a AlbumId,
        cache: CachePolicy,
    ) -> FetchStream<'a, Album> {
        self.albums.load(self, id, cache)
    }
}

async fn fetch_album(metadata: &MetadataClient, id: &AlbumId) -> Result<Album> {
    let address = resource_id(ResourceKind::Album, id.as_str())?;
    let mut resource: AlbumResource = metadata.resource(&address).await?;
    let mut album = mapping::album(&resource)?;
    let page = resource.tracks.take().ok_or(Error::InvalidData {
        field: "album tracks relationship",
    })?;
    let mut tracks = page.data;
    let mut next = page.next;
    while let Some(continuation) = next {
        let page = metadata
            .page(Request::Next {
                continuation: &continuation,
                kind: PageKind::AlbumTracks(address.scope()),
                limit: None,
            })
            .await?;
        tracks.extend(page.data);
        next = page.next;
    }
    if address.scope() == ResourceScope::Catalog {
        metadata
            .hydrate_songs(
                tracks.iter_mut().filter_map(|track| match track {
                    TrackResource::Song(song) => Some(song),
                    TrackResource::Unavailable(_) => None,
                }),
                SongRelationships::Artists,
            )
            .await?;
    }
    for (resource, index) in tracks.iter().zip(0_u64..) {
        let TrackResource::Song(resource) = resource else {
            continue;
        };
        let mut song = mapping::song(resource);
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
