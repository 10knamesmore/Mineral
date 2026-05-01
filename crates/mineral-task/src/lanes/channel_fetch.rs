//! ChannelFetch lane:per-channel 多 worker,两档优先级。

use std::collections::HashMap;
use std::sync::Arc;

use mineral_channel_core::MusicChannel;
use mineral_model::{BitRate, SourceKind};
use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::event::TaskEvent;
use crate::id::{Priority, TaskId};
use crate::kind::ChannelFetchKind;
use crate::ongoing::Ongoing;
use crate::outcome::TaskOutcome;

/// 每个 channel 的 worker 数(per-priority queue 共享)。
const WORKERS_PER_CHANNEL: usize = 8;

/// 投递给 worker 的一次任务。
pub(crate) struct Job {
    pub id: TaskId,
    pub kind: ChannelFetchKind,
    pub cancel: CancellationToken,
    pub done_tx: oneshot::Sender<TaskOutcome>,
}

/// 单个 channel 的发送端句柄(暴露给 Scheduler)。
struct ChannelSenders {
    user: mpsc::UnboundedSender<Job>,
    background: mpsc::UnboundedSender<Job>,
}

/// ChannelFetch lane:对外只暴露 [`ChannelFetchLane::dispatch`]。
pub(crate) struct ChannelFetchLane {
    senders: HashMap<SourceKind, ChannelSenders>,
}

impl ChannelFetchLane {
    /// 启动 lane:为每个 channel spawn 一组 worker,各 worker 共享 user/background 队列。
    ///
    /// # Params:
    ///   - `channels`: 源 channel 列表,按 `source()` 去重后入 lane
    ///   - `ongoing`: 共享的中央状态(worker 完成时 `remove(id)`)
    ///   - `event_tx`: 中央事件 buffer 的发送端
    pub fn spawn(
        channels: &[Arc<dyn MusicChannel>],
        ongoing: &Arc<Ongoing>,
        event_tx: &Arc<Mutex<Vec<TaskEvent>>>,
    ) -> Self {
        let mut senders = HashMap::<SourceKind, ChannelSenders>::new();
        for ch in channels {
            let source = ch.source();
            if senders.contains_key(&source) {
                continue;
            }
            let (user_tx, user_rx) = mpsc::unbounded_channel::<Job>();
            let (bg_tx, bg_rx) = mpsc::unbounded_channel::<Job>();
            spawn_worker_pool(ch, user_rx, bg_rx, ongoing, event_tx);
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

/// 给一个 channel 起 [`WORKERS_PER_CHANNEL`] 个 worker,共享 user/bg 两个队列。
///
/// user/bg 两个 mpsc 用 `Arc<Mutex<Receiver>>` 共享给所有 worker —— `mpsc::Receiver`
/// 不能直接 clone,但 worker 是后台 task,持锁等待是可接受的(每次 await 都释放)。
fn spawn_worker_pool(
    channel: &Arc<dyn MusicChannel>,
    user_rx: mpsc::UnboundedReceiver<Job>,
    bg_rx: mpsc::UnboundedReceiver<Job>,
    ongoing: &Arc<Ongoing>,
    event_tx: &Arc<Mutex<Vec<TaskEvent>>>,
) {
    let user = Arc::new(tokio::sync::Mutex::new(user_rx));
    let bg = Arc::new(tokio::sync::Mutex::new(bg_rx));
    for _ in 0..WORKERS_PER_CHANNEL {
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
                    "{e}"
                );
                TaskOutcome::Failed
            }
        },
        ChannelFetchKind::PlaylistTracks { source, id } => {
            match channel.songs_in_playlist(id).await {
                Ok(tracks) => {
                    event_tx.lock().push(TaskEvent::PlaylistTracksFetched {
                        id: id.clone(),
                        tracks,
                    });
                    TaskOutcome::Ok
                }
                Err(e) => {
                    mineral_log::warn!(
                        target: "channel_fetch",
                        ?source,
                        op = "songs_in_playlist",
                        playlist_id = id.as_str(),
                        "{e}"
                    );
                    TaskOutcome::Failed
                }
            }
        }
        ChannelFetchKind::SongUrl { source, song_id } => {
            let ids = [song_id.clone()];
            match channel.song_urls(&ids, BitRate::Higher).await {
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
                            ?source,
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
                        ?source,
                        op = "song_url",
                        song_id = song_id.as_str(),
                        "{e}"
                    );
                    TaskOutcome::Failed
                }
            }
        }
    }
}
