//! ChannelFetch lane:per-channel 多 worker,两档优先级。

use std::sync::Arc;
use std::time::Instant;

use async_stream::try_stream;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use mineral_channel_core::store::{LibraryStore, NamespaceStore};
use mineral_channel_core::{Error, MusicChannel, Result};
use mineral_model::{CachePolicy, SearchKind, SourceKind};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::collection_read::CollectionReader;
use crate::event::{SearchPayload, TaskEvent};
use crate::id::{Priority, TaskId};
use crate::kind::{ChannelFetchKind, ChannelFetchKindTag};
use crate::ongoing::Ongoing;
use crate::outcome::TaskOutcome;
use crate::read_cache::ReadCache;

/// 投递给 worker 的一次任务。
pub(crate) struct Job {
    pub cache: CachePolicy,

    pub read_cache: Arc<ReadCache>,

    /// 任务 id,worker 完成后用于从 [`Ongoing`] 移除。
    pub id: TaskId,

    /// 业务参数。
    pub kind: ChannelFetchKind,

    /// 触发优先级(埋点 fetches.trigger:User=用户触发、Background=系统链路)。
    pub priority: Priority,

    /// 取消令牌,被 cancel 后 worker 在下一个 await 点直接返回 `Cancelled`。
    pub cancel: CancellationToken,

    /// 终态通知通道(写一次)。
    pub done_tx: oneshot::Sender<TaskOutcome>,
}

/// 单个 channel 的发送端句柄(暴露给 Scheduler)。
struct ChannelSenders {
    /// User 优先级队列发送端。
    user: mpsc::UnboundedSender<Job>,

    /// Background 优先级队列发送端。
    background: mpsc::UnboundedSender<Job>,
}

/// ChannelFetch lane:对外只暴露 [`ChannelFetchLane::dispatch`]。
pub(crate) struct ChannelFetchLane {
    /// 每个 channel 一对发送端;`dispatch` 时按 [`SourceKind`] 路由。
    senders: FxHashMap<SourceKind, ChannelSenders>,
}

impl ChannelFetchLane {
    /// 启动 lane:为每个 channel spawn 一组 worker,各 worker 共享 user/background 队列。
    ///
    /// # Params:
    ///   - `channels`: 源 channel 列表,按 `source()` 去重后入 lane
    ///   - `ongoing`: 共享的中央状态(worker 完成时 `remove(id)`)
    ///   - `event_tx`: 中央事件 buffer(纯 [`TaskEvent`] 事实)
    ///   - `workers_per_channel`: 每个 channel 的 worker 数(user/bg 两级队列共享)
    pub fn spawn(
        channels: &[Arc<dyn MusicChannel>],
        ongoing: &Arc<Ongoing>,
        event_tx: &Arc<Mutex<Vec<TaskEvent>>>,
        workers_per_channel: usize,
        stores: &[Arc<dyn NamespaceStore>],
        library: Option<Arc<dyn LibraryStore>>,
    ) -> Self {
        let reader = Arc::new(CollectionReader::new(channels, stores, library));
        let mut senders = FxHashMap::<SourceKind, ChannelSenders>::default();
        for ch in channels {
            let source = ch.source();
            if senders.contains_key(&source) {
                continue;
            }
            let (user_tx, user_rx) = mpsc::unbounded_channel::<Job>();
            let (bg_tx, bg_rx) = mpsc::unbounded_channel::<Job>();
            spawn_worker_pool(
                ch,
                user_rx,
                bg_rx,
                ongoing,
                event_tx,
                workers_per_channel,
                &reader,
            );
            senders.insert(
                source,
                ChannelSenders {
                    user: user_tx,
                    background: bg_tx,
                },
            );
        }
        Self { senders }
    }

    /// 把一个 [`Job`] 投递到对应 channel 的对应优先级队列。
    /// channel 没注册时,job 直接以 `Failed` 结束(目标 channel 不可达)。
    pub fn dispatch(&self, source: SourceKind, priority: Priority, job: Job) {
        let Some(senders) = self.senders.get(&source) else {
            mineral_log::warn!(target: "channel_fetch", ?source, "no channel registered");
            let _ = job.done_tx.send(TaskOutcome::Failed);
            return;
        };
        let tx = match priority {
            Priority::User => &senders.user,
            Priority::Background => &senders.background,
        };
        let _ = tx.send(job);
    }
}

/// 给一个 channel 起 `workers_per_channel` 个 worker,共享 user/bg 两个队列。
///
/// user/bg 两个 mpsc 用 `Arc<Mutex<Receiver>>` 共享给所有 worker —— `mpsc::Receiver`
/// 不能直接 clone,但 worker 是后台 task,持锁等待是可接受的(每次 await 都释放)。
fn spawn_worker_pool(
    channel: &Arc<dyn MusicChannel>,
    user_rx: mpsc::UnboundedReceiver<Job>,
    bg_rx: mpsc::UnboundedReceiver<Job>,
    ongoing: &Arc<Ongoing>,
    event_tx: &Arc<Mutex<Vec<TaskEvent>>>,
    workers_per_channel: usize,
    reader: &Arc<CollectionReader>,
) {
    let user = Arc::new(tokio::sync::Mutex::new(user_rx));
    let bg = Arc::new(tokio::sync::Mutex::new(bg_rx));
    for _ in 0..workers_per_channel {
        let channel = Arc::clone(channel);
        let user = Arc::clone(&user);
        let bg = Arc::clone(&bg);
        let ongoing = Arc::clone(ongoing);
        let event_tx = Arc::clone(event_tx);
        let reader = Arc::clone(reader);
        tokio::spawn(async move {
            worker_loop(channel, user, bg, ongoing, event_tx, reader).await;
        });
    }
}

/// 单个 worker 的主循环:从 user/bg 队列拉 job 跑完,然后从 ongoing 摘掉。两个队列都关时退出。
async fn worker_loop(
    channel: Arc<dyn MusicChannel>,
    user: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Job>>>,
    bg: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Job>>>,
    ongoing: Arc<Ongoing>,
    event_tx: Arc<Mutex<Vec<TaskEvent>>>,
    reader: Arc<CollectionReader>,
) {
    loop {
        let Some(job) = next_job(&user, &bg).await else {
            return; // 两个队列都关了
        };
        let id = job.id;
        mineral_log::debug!(target: "channel_fetch", task_id = ?id, kind = ?job.kind, "worker start job");
        run_job(&channel, job, &event_tx, &reader).await;
        ongoing.remove(id);
    }
}

/// 偏好 user 队列;user 空了才从 bg 队列拉;两个都空就 await user.recv()(同时 race bg)。
async fn next_job(
    user: &Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Job>>>,
    bg: &Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Job>>>,
) -> Option<Job> {
    // 锁是 worker 私有的 await,持锁短;所有 worker 同时 await recv() 时,
    // tokio mpsc 自己保证只唤醒一个。
    let mut user_g = user.lock().await;
    if let Ok(job) = user_g.try_recv() {
        return Some(job);
    }
    let mut bg_g = bg.lock().await;
    if let Ok(job) = bg_g.try_recv() {
        return Some(job);
    }
    tokio::select! {
        biased;
        j = user_g.recv() => j,
        j = bg_g.recv() => j,
    }
}

/// 逐项发布执行结果；取消或失败后停止消费，发布终态，再通知 done_tx。
/// 搜索与艺人专辑页在失败或取消时均回带请求；详情只在失败时发布失败事件。
async fn run_job(
    channel: &Arc<dyn MusicChannel>,
    job: Job,
    published: &Arc<Mutex<Vec<TaskEvent>>>,
    reader: &CollectionReader,
) {
    let Job {
        id: _,
        kind,
        priority,
        cancel,
        done_tx,
        cache,
        read_cache,
    } = job;
    let started = Instant::now();
    let mut results = execute(channel.as_ref(), &kind, cache, reader);
    let outcome = loop {
        let result = tokio::select! {
            biased;
            () = cancel.cancelled() => break TaskOutcome::Cancelled,
            result = results.next() => result,
        };
        match result {
            Some(Ok(event)) => {
                // 刷新也在同一把锁内取消任务并清除旧结果。
                let mut published = published.lock();
                if cancel.is_cancelled() {
                    break TaskOutcome::Cancelled;
                }
                if cache == CachePolicy::Reload {
                    read_cache.record_fresh_result(&kind, &event);
                }
                published.push(event);
            }
            Some(Err(error)) => {
                if matches!(&kind, ChannelFetchKind::RemotePlayCount { .. }) {
                    mineral_log::debug!(target: "channel_fetch", ?kind, error = mineral_log::chain(&error), "remote play count unavailable");
                } else {
                    mineral_log::warn!(target: "channel_fetch", ?kind, error = mineral_log::chain(&error), "channel fetch failed");
                }
                break TaskOutcome::Failed;
            }
            None => break TaskOutcome::Ok,
        }
    };
    drop(results);
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut published = published.lock();
    let outcome = if cancel.is_cancelled() {
        TaskOutcome::Cancelled
    } else {
        outcome
    };
    if outcome != TaskOutcome::Ok {
        match &kind {
            ChannelFetchKind::PlaylistDetail { id, load } if outcome == TaskOutcome::Failed => {
                published.push(TaskEvent::PlaylistDetailFailed {
                    id: id.clone(),
                    load: *load,
                })
            }
            ChannelFetchKind::Search {
                source,
                kind,
                query,
                page,
            } => published.push(TaskEvent::SearchPageFailed {
                source: *source,
                kind: *kind,
                query: query.clone(),
                page: *page,
            }),
            ChannelFetchKind::AlbumDetail { id } if outcome == TaskOutcome::Failed => {
                published.push(TaskEvent::AlbumDetailFailed { id: id.clone() });
            }
            ChannelFetchKind::ArtistAlbums { id, page } => {
                published.push(TaskEvent::ArtistAlbumsPageFailed {
                    id: id.clone(),
                    page: *page,
                });
            }
            _ => {}
        }
    }
    // 埋点信号:取数收束(fetches)。server 记录后不转发 client。
    published.push(TaskEvent::FetchDone {
        kind: ChannelFetchKindTag::of(&kind),
        source: kind.source(),
        target_ref: kind.target_ref(),
        from_user: priority == Priority::User,
        outcome,
        latency_ms,
    });
    let _ = done_tx.send(outcome);
}

type FetchEventStream<'a> = BoxStream<'a, Result<TaskEvent>>;

/// 将 channel 结果映射为事件；列表可产出多份快照，其他请求产出一个结果。
/// 队列发布、取消和任务终态由调用方处理。
fn execute<'a>(
    channel: &'a dyn MusicChannel,
    kind: &'a ChannelFetchKind,
    cache: CachePolicy,
    reader: &'a CollectionReader,
) -> FetchEventStream<'a> {
    Box::pin(try_stream! {
        match kind {
            ChannelFetchKind::MyAlbums { source } => {
                let mut snapshots = channel.my_albums(cache);
                while let Some(snapshot) = snapshots.next().await {
                    yield TaskEvent::AlbumsFetched {
                        source: *source,
                        albums: snapshot?.into_items(),
                    };
                }
            }
            ChannelFetchKind::MyPlaylists { source } => {
                let mut snapshots = channel.my_playlists(cache);
                while let Some(snapshot) = snapshots.next().await {
                    yield TaskEvent::PlaylistsFetched {
                        source: *source,
                        playlists: snapshot?.into_items(),
                    };
                }
            }
            ChannelFetchKind::PlaylistDetail { id, load } => {
                let detail = reader.playlist(channel, id, *load, cache).await?;
                yield TaskEvent::PlaylistDetailFetched {
                    id: id.clone(),
                    load: *load,
                    detail: Box::new(detail),
                };
            }
            ChannelFetchKind::Lyrics { song_id } => {
                let lyrics = channel.lyrics(song_id).await?;
                yield TaskEvent::LyricsReady {
                    song_id: song_id.clone(),
                    lyrics,
                };
            }
            ChannelFetchKind::RemotePlayCount { song_id } => {
                let count = channel.remote_play_count(song_id).await?;
                yield TaskEvent::RemotePlayCountFetched {
                    song_id: song_id.clone(),
                    count,
                };
            }
            ChannelFetchKind::Search {
                source,
                kind,
                query,
                page,
            } => {
                let (payload, has_more) = match kind {
                    SearchKind::Song => {
                        let hits = channel.search_songs(query, *page).await?;
                        (SearchPayload::Songs(hits.items), hits.has_more)
                    }
                    SearchKind::Album => {
                        let hits = channel.search_albums(query, *page).await?;
                        (SearchPayload::Albums(hits.items), hits.has_more)
                    }
                    SearchKind::Playlist => {
                        let hits = channel.search_playlists(query, *page).await?;
                        (SearchPayload::Playlists(hits.items), hits.has_more)
                    }
                    SearchKind::Artist => {
                        let hits = channel.search_artists(query, *page).await?;
                        (SearchPayload::Artists(hits.items), hits.has_more)
                    }
                    SearchKind::User => Err(Error::NotSupported)?,
                };
                yield TaskEvent::SearchResults {
                    source: *source,
                    kind: *kind,
                    query: query.clone(),
                    page: *page,
                    payload,
                    has_more,
                };
            }
            ChannelFetchKind::ArtistDetail { id } => {
                let artist = channel.artist_detail(id).await?;
                yield TaskEvent::ArtistDetailFetched {
                    id: id.clone(),
                    artist: Box::new(artist),
                };
            }
            ChannelFetchKind::ArtistAlbums { id, page } => {
                let result = channel.artist_albums(id, *page).await?;
                mineral_log::debug!(
                    target: "channel_fetch",
                    source = ?id.namespace(),
                    artist_id = id.as_str(),
                    ?page,
                    albums = result.items.len(),
                    has_more = ?result.has_more,
                    "artist albums page fetched"
                );
                yield TaskEvent::ArtistAlbumsFetched {
                    id: id.clone(),
                    page: *page,
                    albums: result.items,
                    has_more: result.has_more,
                };
            }
            ChannelFetchKind::AlbumDetail { id } => {
                let album = reader.album(channel, id, cache).await?;
                yield TaskEvent::AlbumDetailFetched {
                    id: id.clone(),
                    album: Box::new(album),
                };
            }
        }
    })
}
