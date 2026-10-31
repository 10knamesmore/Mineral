//! Snapshots produced while reading a channel resource.

use std::future::{Future, ready};

use async_stream::try_stream;
use futures_util::TryStreamExt;
use futures_util::stream::BoxStream;

use crate::Result;

/// Freshness is independent of how much of a resource has been loaded.
#[derive(Debug)]
pub enum FetchSnapshot<T> {
    /// A previous snapshot for display while the source is being refreshed.
    Cached(T),

    /// The requested read is ready: an unexpired cache or a successful source fetch.
    Fresh(T),
}

impl<T> FetchSnapshot<T> {
    /// Whether this snapshot satisfies the channel's freshness policy.
    pub fn is_fresh(&self) -> bool {
        matches!(self, Self::Fresh(_))
    }

    /// Consumes the value without retaining its freshness.
    pub fn into_value(self) -> T {
        match self {
            Self::Cached(value) | Self::Fresh(value) => value,
        }
    }
}

/// Optional cached snapshots followed by a fresh result or a terminal error.
/// Poll to completion to finish the read; dropping the stream cancels further work.
pub type FetchStream<'a, T> = BoxStream<'a, Result<FetchSnapshot<T>>>;

/// Adapts a read without intermediate snapshots.
pub fn fresh_snapshot<'a, T: Send + 'a>(
    fetch: impl Future<Output = Result<T>> + Send + 'a,
) -> FetchStream<'a, T> {
    Box::pin(try_stream! {
        yield FetchSnapshot::Fresh(fetch.await?);
    })
}

/// Consumers that need one final value wait for freshness rather than using a stale preview.
pub async fn finish_fetch<T: Send>(stream: FetchStream<'_, T>) -> Result<Option<T>> {
    stream
        .try_fold(None, |result, snapshot| {
            ready(Ok(match snapshot {
                FetchSnapshot::Cached(_) => result,
                FetchSnapshot::Fresh(value) => Some(value),
            }))
        })
        .await
}
