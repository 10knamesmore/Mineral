//! 将来源的四类搜索批次映射成客户端共用的搜索载荷。

use futures_util::StreamExt;
use futures_util::stream::{self, BoxStream};
use mineral_channel_core::{BatchStream, Error, MusicChannel, Result};
use mineral_model::SearchKind;

use crate::SearchPayload;

/// 一次推进取得的来源结果。
pub struct SearchBatch {
    /// 与搜索类型匹配的结果条目。
    pub payload: SearchPayload,

    /// 此搜索已经没有后续批次。
    pub complete: bool,
}

/// 已映射公共实体的搜索流；分页进度仍由来源持有。
pub type SearchStream<'a> = BoxStream<'a, Result<SearchBatch>>;

pub(crate) fn read_search<'a>(
    channel: &'a dyn MusicChannel,
    kind: SearchKind,
    query: &'a str,
) -> SearchStream<'a> {
    fn map<'a, T: Send + 'a>(
        stream: BatchStream<'a, T>,
        payload: fn(Vec<T>) -> SearchPayload,
    ) -> SearchStream<'a> {
        Box::pin(stream.map(move |result| {
            result.map(|batch| SearchBatch {
                payload: payload(batch.items),
                complete: batch.complete,
            })
        }))
    }
    match kind {
        SearchKind::Song => map(channel.search_songs(query), SearchPayload::Songs),
        SearchKind::Album => map(channel.search_albums(query), SearchPayload::Albums),
        SearchKind::Playlist => map(channel.search_playlists(query), SearchPayload::Playlists),
        SearchKind::Artist => map(channel.search_artists(query), SearchPayload::Artists),
        SearchKind::User => Box::pin(stream::once(async { Err(Error::NotSupported) })),
    }
}
