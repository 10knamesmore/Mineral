//! ChannelFetch lane:per-channel 多 worker,两档优先级。

use std::future::Future;
use std::result::Result as StdResult;
use std::sync::Arc;
use std::time::Instant;

use async_stream::{stream, try_stream};
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use mineral_channel_core::store::{LibraryStore, NamespaceStore};
use mineral_channel_core::{
    BatchStream, Error, FetchSnapshot, FetchStream, MusicChannel, PlaylistDetail, Result,
};
use mineral_model::{Album, ArtistId, CachePolicy, PlaylistId, SearchKind, SourceKind};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use thiserror::Error as ThisError;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::collection_read::CollectionReader;
use crate::event::TaskEvent;
use crate::id::{Priority, TaskId};
use crate::kind::{ChannelFetchKind, ChannelFetchKindTag};
use crate::ongoing::Ongoing;
use crate::outcome::TaskOutcome;
use crate::read_cache::ReadCache;
use crate::search::{SearchStream, read_search};

/// The source execution queue is unavailable or has stopped.
#[derive(Debug, ThisError)]
#[error("channel execution queue unavailable for {channel:?}")]
pub struct DispatchError {
    /// Source whose queue could not grant execution.
    pub channel: SourceKind,
}

/// A queued fetch or a single execution turn owned by an existing caller.
enum Work {
    /// A finite task owned by the queue.
    Fetch(Job),

    /// The worker grants a release sender; the caller drops it after one operation.
    Turn(oneshot::Sender<oneshot::Sender<()>>),
}

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
    /// Used to create lazy streams without transferring their ownership to a worker.
    channel: Arc<dyn MusicChannel>,

    /// User 优先级队列发送端。
    user: mpsc::UnboundedSender<Work>,

    /// Background 优先级队列发送端。
    background: mpsc::UnboundedSender<Work>,
}

/// 来源执行队列，有限任务和订阅推进共用 worker 容量与优先级。
pub(crate) struct ChannelFetchLane {
    /// 每个 channel 一对发送端;`dispatch` 时按 [`SourceKind`] 路由。
    senders: FxHashMap<SourceKind, ChannelSenders>,

    /// Applies shared metadata refresh and persistence to detail streams.
    reader: Arc<CollectionReader>,
}

impl ChannelFetchLane {
    /// 生成器持有艺人身份和来源，跨批次保留来源游标。
    pub fn artist_albums(&self, id: ArtistId) -> BatchStream<'static, Album> {
        let channel = self
            .senders
            .get(&id.namespace())
            .map(|senders| Arc::clone(&senders.channel));
        Box::pin(stream! {
            let Some(channel) = channel else {
                yield Err(Error::NotSupported);
                return;
            };
            let mut batches = channel.artist_albums(&id);
            while let Some(batch) = batches.next().await {
                yield batch;
            }
        })
    }

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
            let (user_tx, user_rx) = mpsc::unbounded_channel();
            let (bg_tx, bg_rx) = mpsc::unbounded_channel();
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
                    channel: Arc::clone(ch),
                    user: user_tx,
                    background: bg_tx,
                },
            );
        }
        Self { senders, reader }
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
        let _ = tx.send(Work::Fetch(job));
    }

    /// Grants one source execution turn. Dropping this future releases the turn;
    /// the caller retains its stream between turns, outside the worker queue.
    pub async fn run<T>(
        &self,
        source: SourceKind,
        priority: Priority,
        operation: impl Future<Output = T>,
    ) -> StdResult<T, DispatchError> {
        let senders = self
            .senders
            .get(&source)
            .ok_or(DispatchError { channel: source })?;
        let queue = match priority {
            Priority::User => &senders.user,
            Priority::Background => &senders.background,
        };
        let (ready, waiting) = oneshot::channel();
        queue
            .send(Work::Turn(ready))
            .map_err(|_closed| DispatchError { channel: source })?;
        let turn = waiting
            .await
            .map_err(|_closed| DispatchError { channel: source })?;
        let result = operation.await;
        drop(turn);
        Ok(result)
    }

    /// Owns the channel and query so the borrowed source stream stays inside its generator.
    pub fn playlist(
        &self,
        id: PlaylistId,
        cache: CachePolicy,
    ) -> FetchStream<'static, PlaylistDetail> {
        let channel = self
            .senders
            .get(&id.namespace())
            .map(|senders| Arc::clone(&senders.channel));
        let reader = Arc::clone(&self.reader);
        Box::pin(try_stream! {
            let channel = channel.ok_or(Error::NotSupported)?;
            let mut snapshots = reader.playlist(channel.as_ref(), &id, cache);
            while let Some(snapshot) = snapshots.next().await {
                yield snapshot?;
            }
        })
    }

    /// 查询和来源由生成器拥有，暂停时不占用 worker。
    pub fn search(
        &self,
        source: SourceKind,
        kind: SearchKind,
        query: String,
    ) -> SearchStream<'static> {
        let channel = self
            .senders
            .get(&source)
            .map(|senders| Arc::clone(&senders.channel));
        Box::pin(stream! {
            let Some(channel) = channel else {
                yield Err(Error::NotSupported);
                return;
            };
            let mut batches = read_search(channel.as_ref(), kind, &query);
            while let Some(batch) = batches.next().await {
                yield batch;
            }
        })
    }
}

/// 给一个 channel 起 `workers_per_channel` 个 worker,共享 user/bg 两个队列。
///
/// user/bg 两个 mpsc 用 `Arc<Mutex<Receiver>>` 共享给所有 worker —— `mpsc::Receiver`
/// 不能直接 clone,但 worker 是后台 task,持锁等待是可接受的(每次 await 都释放)。
fn spawn_worker_pool(
    channel: &Arc<dyn MusicChannel>,
    user_rx: mpsc::UnboundedReceiver<Work>,
    bg_rx: mpsc::UnboundedReceiver<Work>,
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
    user: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Work>>>,
    bg: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Work>>>,
    ongoing: Arc<Ongoing>,
    event_tx: Arc<Mutex<Vec<TaskEvent>>>,
    reader: Arc<CollectionReader>,
) {
    loop {
        let Some(job) = next_job(&user, &bg).await else {
            return; // 两个队列都关了
        };
        match job {
            Work::Fetch(job) => {
                let id = job.id;
                mineral_log::debug!(target: "channel_fetch", task_id = ?id, kind = ?job.kind, "worker start job");
                run_job(&channel, job, &event_tx, &reader).await;
                ongoing.remove(id);
            }
            Work::Turn(ready) => {
                let (release, released) = oneshot::channel();
                if ready.send(release).is_ok() {
                    let _ = released.await;
                }
            }
        }
    }
}

/// 偏好 user 队列;user 空了才从 bg 队列拉;两个都空就 await user.recv()(同时 race bg)。
async fn next_job(
    user: &Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Work>>>,
    bg: &Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Work>>>,
) -> Option<Work> {
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
            ChannelFetchKind::PlaylistDetail { id } if outcome == TaskOutcome::Failed => {
                published.push(TaskEvent::PlaylistDetailFailed { id: id.clone() })
            }
            ChannelFetchKind::Search {
                source,
                kind,
                query,
            } => published.push(TaskEvent::SearchPageFailed {
                source: *source,
                kind: *kind,
                query: query.clone(),
                batch: 0,
            }),
            ChannelFetchKind::AlbumDetail { id } if outcome == TaskOutcome::Failed => {
                published.push(TaskEvent::AlbumDetailFailed { id: id.clone() });
            }
            ChannelFetchKind::ArtistAlbums { id } => {
                published.push(TaskEvent::ArtistAlbumsPageFailed {
                    id: id.clone(),
                    batch: 0,
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
                        albums: snapshot?.into_value(),
                    };
                }
            }
            ChannelFetchKind::MyPlaylists { source } => {
                let mut snapshots = channel.my_playlists(cache);
                while let Some(snapshot) = snapshots.next().await {
                    yield TaskEvent::PlaylistsFetched {
                        source: *source,
                        playlists: snapshot?.into_value(),
                    };
                }
            }
            ChannelFetchKind::PlaylistDetail { id } => {
                let mut snapshots = reader.playlist(channel, id, cache);
                while let Some(snapshot) = snapshots.next().await {
                    let snapshot = snapshot?;
                    let complete = matches!(&snapshot, FetchSnapshot::Fresh(detail) if detail.complete);
                    if snapshot.is_fresh() && !complete {
                        continue;
                    }
                    yield TaskEvent::PlaylistDetailFetched {
                        id: id.clone(),
                        fresh: snapshot.is_fresh(),
                        detail: Box::new(snapshot.into_value()),
                    };
                    if complete {
                        return;
                    }
                }
                Err(Error::InvalidData { field: "playlist stream ended before complete" })?;
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
            } => {
                let result = read_search(channel, *kind, query).next().await
                    .ok_or(Error::InvalidData { field: "search stream ended before complete" })??;
                yield TaskEvent::SearchResults {
                    source: *source,
                    kind: *kind,
                    query: query.clone(),
                    batch: 0,
                    payload: result.payload,
                    complete: result.complete,
                };
            }
            ChannelFetchKind::ArtistDetail { id } => {
                let artist = channel.artist_detail(id).await?;
                yield TaskEvent::ArtistDetailFetched {
                    id: id.clone(),
                    artist: Box::new(artist),
                };
            }
            ChannelFetchKind::ArtistAlbums { id } => {
                let result = channel.artist_albums(id).next().await
                    .ok_or(Error::InvalidData { field: "artist albums stream ended before complete" })??;
                yield TaskEvent::ArtistAlbumsFetched {
                    id: id.clone(),
                    batch: 0,
                    albums: result.items,
                    complete: result.complete,
                };
            }
            ChannelFetchKind::AlbumDetail { id } => {
                let mut snapshots = reader.album(channel, id, cache);
                while let Some(snapshot) = snapshots.next().await {
                    let snapshot = snapshot?;
                    yield TaskEvent::AlbumDetailFetched {
                        id: id.clone(),
                        fresh: snapshot.is_fresh(),
                        album: Box::new(snapshot.into_value()),
                    };
                }
            }
        }
    })
}
