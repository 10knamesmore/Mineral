//! Cache-first collection reads driven by the caller, without detached refresh tasks.

use std::future::Future;

use async_stream::try_stream;
use futures_util::stream::BoxStream;
use mineral_model::{Album, CachePolicy, Playlist};

use crate::store::{NamespaceStore, StoreError};
use crate::{Error, Result};

/// A complete ordered list, not a page or a change set.
#[derive(Debug)]
pub enum CollectionSnapshot<T> {
    /// The previous successful read, published before refreshing the source.
    Cached(Vec<T>),

    /// The current source result, published after its cache write completes.
    Fresh(Vec<T>),
}

impl<T> CollectionSnapshot<T> {
    /// Consumes the snapshot without retaining its freshness.
    pub fn into_items(self) -> Vec<T> {
        match self {
            Self::Cached(items) | Self::Fresh(items) => items,
        }
    }
}

/// An optional cached list followed by a fresh list, or a terminal error.
/// Poll until exhaustion to finish refreshing; dropping the stream cancels further work.
pub type CollectionStream<'a, T> = BoxStream<'a, Result<CollectionSnapshot<T>>>;

/// Adapts an authoritative read that does not need a separate list cache.
pub fn fresh_collection<'a, T: Send + 'a>(
    fetch: impl Future<Output = Result<Vec<T>>> + Send + 'a,
) -> CollectionStream<'a, T> {
    Box::pin(try_stream! {
        yield CollectionSnapshot::Fresh(fetch.await?);
    })
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
            yield CollectionSnapshot::Cached(playlists);
        }
        let playlists = fetch.await?;
        if let Some(store) = store {
            store.put_my_playlists_cache(&playlists).await.map_err(storage_error)?;
        }
        yield CollectionSnapshot::Fresh(playlists);
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
            yield CollectionSnapshot::Cached(albums);
        }
        let albums = fetch.await?;
        if let Some(store) = store {
            store.put_my_albums_cache(&albums).await.map_err(storage_error)?;
        }
        yield CollectionSnapshot::Fresh(albums);
    })
}

fn storage_error(source: StoreError) -> Error {
    Error::Storage {
        source: Box::new(source),
    }
}
