//! Cache-first collection reads driven by the caller, without detached refresh tasks.

use std::future::Future;

use async_stream::try_stream;
use mineral_model::{Album, CachePolicy, Playlist};

use crate::fetch::{FetchSnapshot, FetchStream, fresh_snapshot};
use crate::store::{NamespaceStore, StoreError};
use crate::{Error, Result};

/// An optional cached list followed by a fresh list, or a terminal error.
/// Poll until exhaustion to finish refreshing; dropping the stream cancels further work.
pub type CollectionStream<'a, T> = FetchStream<'a, Vec<T>>;

/// Adapts an authoritative read that does not need a separate list cache.
pub fn fresh_collection<'a, T: Send + 'a>(
    fetch: impl Future<Output = Result<Vec<T>>> + Send + 'a,
) -> CollectionStream<'a, T> {
    fresh_snapshot(fetch)
}

/// Publishes stored playlist headers, then replaces them with a complete source result.
pub fn playlists_with_cache<'a>(
    store: Option<&'a dyn NamespaceStore>,
    policy: CachePolicy,
    fetch: impl Future<Output = Result<Vec<Playlist>>> + Send + 'a,
) -> CollectionStream<'a, Playlist> {
    Box::pin(try_stream! {
        let cached = match (policy, store) {
            (CachePolicy::UseCache, Some(store)) => {
                store.get_my_playlists_cache().await.map_err(storage_error)?
            }
            _ => None,
        };
        if let Some(playlists) = cached {
            yield FetchSnapshot::Cached(playlists);
        }
        let playlists = fetch.await?;
        if let Some(store) = store {
            store.put_my_playlists_cache(&playlists).await.map_err(storage_error)?;
        }
        yield FetchSnapshot::Fresh(playlists);
    })
}

/// Publishes stored album headers, then replaces them after every source page succeeds.
pub fn albums_with_cache<'a>(
    store: Option<&'a dyn NamespaceStore>,
    policy: CachePolicy,
    fetch: impl Future<Output = Result<Vec<Album>>> + Send + 'a,
) -> CollectionStream<'a, Album> {
    Box::pin(try_stream! {
        let cached = match (policy, store) {
            (CachePolicy::UseCache, Some(store)) => {
                store.get_my_albums_cache().await.map_err(storage_error)?
            }
            _ => None,
        };
        if let Some(albums) = cached {
            yield FetchSnapshot::Cached(albums);
        }
        let albums = fetch.await?;
        if let Some(store) = store {
            store.put_my_albums_cache(&albums).await.map_err(storage_error)?;
        }
        yield FetchSnapshot::Fresh(albums);
    })
}

fn storage_error(source: StoreError) -> Error {
    Error::Storage {
        source: Box::new(source),
    }
}
