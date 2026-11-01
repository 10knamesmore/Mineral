//! Lua 搜索句柄拥有读取请求通道；每次 next 推进各来源一批，关闭时释放全部流。

use async_stream::stream;
use futures_util::StreamExt;
use mineral_channel_core::{BatchStream, Error};
use mineral_model::{SearchKind, Song, SourceKind};
use mineral_script::{QueryId, ResolveValue};
use mineral_task::{ChannelFetchKind, Priority};
use tokio::sync::{mpsc, oneshot};

use crate::player::PlayerCore;
use crate::script_bridge::{record_script_search, resolve_err, resolve_ok};

/// 一个来源在当前脚本搜索中的进度。
struct SourceSearch {
    /// 来源执行队列和统计归属。
    source: SourceKind,

    /// 用于统计的成功批次序号。
    batch: u64,

    /// 来源保存的分页进度。
    stream: BatchStream<'static, Song>,
}

#[derive(Debug, thiserror::Error)]
enum SearchError {
    /// 指定来源未注册。
    #[error("no channel for source {0:?}")]
    MissingSource(SourceKind),

    /// 句柄已经关闭或读完，尚未处理的 next 不再执行。
    #[error("search is closed")]
    Closed,
}

pub(crate) async fn run(
    player: PlayerCore,
    term: String,
    source: Option<SourceKind>,
    mut requests: mpsc::UnboundedReceiver<QueryId>,
    mut closed: oneshot::Receiver<()>,
) {
    let scheduler = &player.inner.scheduler;
    let mut sources = player
        .channels()
        .iter()
        .filter(|channel| source.is_none_or(|source| source == channel.source()))
        .map(|channel| {
            let source = channel.source();
            let channel = channel.clone();
            let term = term.clone();
            SourceSearch {
                source,
                batch: 0,
                stream: Box::pin(stream! {
                    let mut batches = channel.search_songs(&term);
                    while let Some(batch) = batches.next().await {
                        yield batch;
                    }
                }),
            }
        })
        .collect::<Vec<_>>();
    loop {
        let query = tokio::select! {
            biased;
            _ = &mut closed => break,
            query = requests.recv() => match query {
                Some(query) => query,
                None => break,
            },
        };
        if let Some(source) = source
            && player.channel_for(source).is_none()
        {
            resolve_err(&player, query, SearchError::MissingSource(source));
            continue;
        }
        let read = async {
            let mut items = Vec::new();
            let mut index = 0;
            while let Some(current) = sources.get_mut(index) {
                let result = scheduler
                    .run_channel(
                        ChannelFetchKind::Search {
                            source: current.source,
                            kind: SearchKind::Song,
                            query: term.clone(),
                        },
                        Priority::User,
                        async {
                            current.stream.next().await.ok_or(Error::InvalidData {
                                field: "search stream ended before complete",
                            })?
                        },
                    )
                    .await;
                let page = i64::try_from(current.batch).unwrap_or(i64::MAX);
                match result {
                    Ok(batch) => {
                        record_script_search(
                            &player,
                            &term,
                            current.source,
                            page,
                            Some(batch.items.len()),
                        );
                        items.extend(batch.items);
                        current.batch += 1;
                        if batch.complete {
                            sources.remove(index);
                        } else {
                            index += 1;
                        }
                    }
                    Err(error) => {
                        record_script_search(&player, &term, current.source, page, None);
                        if source.is_some() {
                            return Err(error);
                        }
                        mineral_log::warn!(target: "script", source = current.source.name(),
                            error = mineral_log::chain(&error), "library.search: source failed");
                        sources.remove(index);
                    }
                }
            }
            Ok(items)
        };
        let result = tokio::select! {
            biased;
            _ = &mut closed => {
                resolve_err(&player, query, SearchError::Closed);
                break;
            }
            result = read => result,
        };
        match result {
            Ok(items) => {
                let complete = sources.is_empty();
                resolve_ok(
                    &player,
                    query,
                    ResolveValue::SearchBatch { items, complete },
                );
                if complete {
                    break;
                }
            }
            Err(error) => resolve_err(&player, query, error),
        }
    }
    requests.close();
    while let Some(query) = requests.recv().await {
        resolve_err(&player, query, SearchError::Closed);
    }
}
