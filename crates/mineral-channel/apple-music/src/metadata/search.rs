//! 搜索流保留 Apple 的 continuation，每批补齐关系后映射到公共模型。

use std::future::Future;

use async_stream::stream;
use mineral_channel_core::{Batch, BatchStream};
use mineral_model::{Album, Artist, Playlist, Song};

use super::data::{AlbumResource, ArtistResource, PageCursor, PlaylistResource, SongResource};
use super::request::SongRelationships;
use super::wire::MetadataResource;
use super::{MetadataClient, mapping};
use crate::Result;

impl MetadataClient {
    pub(crate) fn search_playlists<'a>(&'a self, query: &'a str) -> BatchStream<'a, Playlist> {
        self.search(query, |items: Vec<PlaylistResource>| async move {
            items.iter().map(mapping::playlist).collect()
        })
    }

    pub(crate) fn search_songs<'a>(&'a self, query: &'a str) -> BatchStream<'a, Song> {
        self.search(query, move |mut items: Vec<SongResource>| async move {
            self.hydrate_songs(&mut items, SongRelationships::ArtistsAndAlbums)
                .await?;
            Ok(items.iter().map(mapping::song).collect())
        })
    }

    pub(crate) fn search_albums<'a>(&'a self, query: &'a str) -> BatchStream<'a, Album> {
        self.search(query, move |mut items: Vec<AlbumResource>| async move {
            self.hydrate_albums(&mut items).await?;
            items.iter().map(mapping::album).collect()
        })
    }

    pub(crate) fn search_artists<'a>(&'a self, query: &'a str) -> BatchStream<'a, Artist> {
        self.search(query, |items: Vec<ArtistResource>| async move {
            items.iter().map(mapping::artist).collect()
        })
    }

    /// 关系补全失败时仍保留当前页的游标，下一次消费重试同一页。
    fn search<'a, T, M, F, Fut>(&'a self, query: &'a str, mut map: F) -> BatchStream<'a, M>
    where
        T: MetadataResource + Send + 'a,
        M: Send + 'a,
        F: FnMut(Vec<T>) -> Fut + Send + 'a,
        Fut: Future<Output = Result<Vec<M>>> + Send + 'a,
    {
        Box::pin(stream! {
            let mut cursor = PageCursor::First;
            loop {
                let result: Result<_> = async {
                    match self.search_resources::<T>(query, &cursor).await? {
                        Some(page) => Ok((map(page.data).await?, page.next)),
                        None => Ok((Vec::new(), None)),
                    }
                }.await;
                match result {
                    Ok((items, next)) => {
                        let complete = next.is_none();
                        yield Ok(Batch { items, complete });
                        let Some(next) = next else { return };
                        cursor = PageCursor::Next(next);
                    }
                    Err(error) => yield Err(error.into()),
                }
            }
        })
    }
}
