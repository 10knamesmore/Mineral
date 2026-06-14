//! ChannelFetch lane:per-channel 多 worker,两档优先级。

use std::sync::Arc;

use mineral_channel_core::MusicChannel;
use mineral_model::SourceKind;
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::event::TaskEvent;
use crate::id::{Priority, TaskId};
use crate::kind::ChannelFetchKind;
use crate::ongoing::Ongoing;
use crate::outcome::TaskOutcome;

/// 投递给 worker 的一次任务。
pub(crate) struct Job {
    /// 任务 id,worker 完成后用于从 [`Ongoing`] 移除。
    pub id: TaskId,

    /// 业务参数。
    pub kind: ChannelFetchKind,

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
    ///   - `event_tx`: 中央事件 buffer 的发送端
    ///   - `workers_per_channel`: 每个 channel 的 worker 数(user/bg 两级队列共享)
    pub fn spawn(
        channels: &[Arc<dyn MusicChannel>],
        ongoing: &Arc<Ongoing>,
        event_tx: &Arc<Mutex<Vec<TaskEvent>>>,
        workers_per_channel: usize,
    ) -> Self {
        let mut senders = FxHashMap::<SourceKind, ChannelSenders>::default();
        for ch in channels {
            let source = ch.source();
            if senders.contains_key(&source) {
                continue;
            }
            let (user_tx, user_rx) = mpsc::unbounded_channel::<Job>();
            let (bg_tx, bg_rx) = mpsc::unbounded_channel::<Job>();
            spawn_worker_pool(ch, user_rx, bg_rx, ongoing, event_tx, workers_per_channel);
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
) {
    let user = Arc::new(tokio::sync::Mutex::new(user_rx));
    let bg = Arc::new(tokio::sync::Mutex::new(bg_rx));
    for _ in 0..workers_per_channel {
        let channel = Arc::clone(channel);
        let user = Arc::clone(&user);
        let bg = Arc::clone(&bg);
        let ongoing = Arc::clone(ongoing);
        let event_tx = Arc::clone(event_tx);
        tokio::spawn(async move {
            worker_loop(channel, user, bg, ongoing, event_tx).await;
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
) {
    loop {
        let Some(job) = next_job(&user, &bg).await else {
            return; // 两个队列都关了
        };
        let id = job.id;
        mineral_log::debug!(target: "channel_fetch", task_id = ?id, kind = ?job.kind, "worker start job");
        run_job(&channel, job, &event_tx).await;
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

/// 执行一个 job:已取消则直接 `Cancelled`,否则跑 [`execute`] 并把终态送回 `done_tx`。
async fn run_job(channel: &Arc<dyn MusicChannel>, job: Job, event_tx: &Arc<Mutex<Vec<TaskEvent>>>) {
    let Job {
        id: _,
        kind,
        cancel,
        done_tx,
    } = job;
    if cancel.is_cancelled() {
        let _ = done_tx.send(TaskOutcome::Cancelled);
        return;
    }
    let outcome = tokio::select! {
        biased;
        () = cancel.cancelled() => TaskOutcome::Cancelled,
        out = execute(channel, &kind, event_tx) => out,
    };
    let _ = done_tx.send(outcome);
}

/// 真正调 channel 的实现:按 kind 分派,把结果包成 [`TaskEvent`] 写进事件 buffer,失败统一变 `Failed`。
async fn execute(
    channel: &Arc<dyn MusicChannel>,
    kind: &ChannelFetchKind,
    event_tx: &Arc<Mutex<Vec<TaskEvent>>>,
) -> TaskOutcome {
    match kind {
        ChannelFetchKind::MyPlaylists { source } => match channel.my_playlists().await {
            Ok(playlists) => {
                event_tx.lock().push(TaskEvent::PlaylistsFetched {
                    source: *source,
                    playlists,
                });
                TaskOutcome::Ok
            }
            Err(e) => {
                mineral_log::warn!(
                    target: "channel_fetch",
                    ?source,
                    op = "my_playlists",
                    error = mineral_log::chain(&e),
                    "channel fetch failed"
                );
                TaskOutcome::Failed
            }
        },
        ChannelFetchKind::LikedSongIds { source } => match channel.liked_song_ids().await {
            Ok(ids) => {
                event_tx.lock().push(TaskEvent::LikedSongIdsFetched {
                    source: *source,
                    ids,
                });
                TaskOutcome::Ok
            }
            Err(e) => {
                mineral_log::warn!(
                    target: "channel_fetch",
                    ?source,
                    op = "liked_song_ids",
                    error = mineral_log::chain(&e),
                    "channel fetch failed"
                );
                TaskOutcome::Failed
            }
        },
        ChannelFetchKind::PlaylistDetail { id } => match channel.playlist_detail(id).await {
            Ok(playlist) => {
                event_tx.lock().push(TaskEvent::PlaylistDetailFetched {
                    id: id.clone(),
                    playlist: Box::new(playlist),
                });
                TaskOutcome::Ok
            }
            Err(e) => {
                mineral_log::warn!(
                    target: "channel_fetch",
                    source = ?id.namespace(),
                    op = "playlist_detail",
                    playlist_id = id.as_str(),
                    error = mineral_log::chain(&e),
                    "channel fetch failed"
                );
                TaskOutcome::Failed
            }
        },
        ChannelFetchKind::SongUrl { song_id, quality } => {
            let ids = [song_id.clone()];
            match channel.song_urls(&ids, *quality).await {
                Ok(mut urls) => match urls.pop() {
                    Some(play_url) => {
                        event_tx.lock().push(TaskEvent::PlayUrlReady {
                            song_id: song_id.clone(),
                            play_url,
                        });
                        TaskOutcome::Ok
                    }
                    None => {
                        mineral_log::warn!(
                            target: "channel_fetch",
                            source = ?song_id.namespace(),
                            op = "song_url",
                            song_id = song_id.as_str(),
                            "channel returned empty url list"
                        );
                        TaskOutcome::Failed
                    }
                },
                Err(e) => {
                    mineral_log::warn!(
                        target: "channel_fetch",
                        source = ?song_id.namespace(),
                        op = "song_url",
                        song_id = song_id.as_str(),
                        error = mineral_log::chain(&e),
                        "channel fetch failed"
                    );
                    TaskOutcome::Failed
                }
            }
        }
        ChannelFetchKind::Lyrics { song_id } => match channel.lyrics(song_id).await {
            Ok(lyrics) => {
                event_tx.lock().push(TaskEvent::LyricsReady {
                    song_id: song_id.clone(),
                    lyrics,
                });
                TaskOutcome::Ok
            }
            Err(e) => {
                mineral_log::warn!(
                    target: "channel_fetch",
                    source = ?song_id.namespace(),
                    op = "lyrics",
                    song_id = song_id.as_str(),
                    error = mineral_log::chain(&e),
                    "channel fetch failed"
                );
                TaskOutcome::Failed
            }
        },
        ChannelFetchKind::RemotePlayCount { song_id } => {
            match channel.remote_play_count(song_id).await {
                Ok(count) => {
                    event_tx.lock().push(TaskEvent::RemotePlayCountFetched {
                        song_id: song_id.clone(),
                        count,
                    });
                    TaskOutcome::Ok
                }
                Err(e) => {
                    // 装饰性尽力查询:未登录 / 不支持 / 网络失败都无害,debug 即可,不污染 warn。
                    mineral_log::debug!(
                        target: "channel_fetch",
                        source = ?song_id.namespace(),
                        op = "remote_play_count",
                        song_id = song_id.as_str(),
                        error = mineral_log::chain(&e),
                        "remote play count unavailable"
                    );
                    TaskOutcome::Failed
                }
            }
        }
        ChannelFetchKind::Search {
            source,
            kind,
            query,
            page,
        } => {
            let result = match kind {
                mineral_model::SearchKind::Song => channel
                    .search_songs(query, *page)
                    .await
                    .map(crate::event::SearchPayload::Songs),
                mineral_model::SearchKind::Album => channel
                    .search_albums(query, *page)
                    .await
                    .map(crate::event::SearchPayload::Albums),
                mineral_model::SearchKind::Playlist => channel
                    .search_playlists(query, *page)
                    .await
                    .map(crate::event::SearchPayload::Playlists),
                mineral_model::SearchKind::Artist => channel
                    .search_artists(query, *page)
                    .await
                    .map(crate::event::SearchPayload::Artists),
                // User 搜索无 UI 消费方,caps 也不会声明它
                mineral_model::SearchKind::User => Err(mineral_channel_core::Error::NotSupported),
            };
            match result {
                Ok(payload) => {
                    event_tx.lock().push(TaskEvent::SearchResults {
                        source: *source,
                        kind: *kind,
                        query: query.clone(),
                        page: *page,
                        payload,
                    });
                    TaskOutcome::Ok
                }
                Err(e) => {
                    mineral_log::warn!(
                        target: "channel_fetch",
                        ?source,
                        op = "search",
                        ?kind,
                        query,
                        error = mineral_log::chain(&e),
                        "channel fetch failed"
                    );
                    TaskOutcome::Failed
                }
            }
        }
        ChannelFetchKind::ArtistDetail { id } => match channel.artist_detail(id).await {
            Ok(artist) => {
                event_tx.lock().push(TaskEvent::ArtistDetailFetched {
                    id: id.clone(),
                    artist: Box::new(artist),
                });
                TaskOutcome::Ok
            }
            Err(e) => {
                mineral_log::warn!(
                    target: "channel_fetch",
                    source = ?id.namespace(),
                    op = "artist_detail",
                    artist_id = id.as_str(),
                    error = mineral_log::chain(&e),
                    "channel fetch failed"
                );
                TaskOutcome::Failed
            }
        },
        ChannelFetchKind::ArtistAlbums { id, page } => {
            match channel.artist_albums(id, *page).await {
                Ok(albums) => {
                    event_tx.lock().push(TaskEvent::ArtistAlbumsFetched {
                        id: id.clone(),
                        page: *page,
                        albums,
                    });
                    TaskOutcome::Ok
                }
                Err(e) => {
                    mineral_log::warn!(
                        target: "channel_fetch",
                        source = ?id.namespace(),
                        op = "artist_albums",
                        artist_id = id.as_str(),
                        error = mineral_log::chain(&e),
                        "channel fetch failed"
                    );
                    TaskOutcome::Failed
                }
            }
        }
        ChannelFetchKind::AlbumDetail { id } => match channel.album_detail(id).await {
            Ok(album) => {
                event_tx.lock().push(TaskEvent::AlbumDetailFetched {
                    id: id.clone(),
                    album: Box::new(album),
                });
                TaskOutcome::Ok
            }
            Err(e) => {
                mineral_log::warn!(
                    target: "channel_fetch",
                    source = ?id.namespace(),
                    op = "album_detail",
                    album_id = id.as_str(),
                    error = mineral_log::chain(&e),
                    "channel fetch failed"
                );
                TaskOutcome::Failed
            }
        },
    }
}
