//! PlaylistWrite lane:per-source **单 worker 严格串行**。
//!
//! 与 ChannelFetch lane 的两点刻意差异:
//! 1. 单 worker 单队列——同源两个写操作乱序到达远端会丢更新,串行是
//!    顺序保证,不是性能取舍;不同源之间仍然并发。
//! 2. **开跑后不再响应取消**——写请求一旦发出,远端可能已经执行,中途
//!    cancel 只会让本地状态与远端脱节;取消窗口只在排队期(开跑前检查一次)。

use std::sync::Arc;

use mineral_channel_core::MusicChannel;
use mineral_model::SourceKind;
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::event::TaskEvent;
use crate::id::TaskId;
use crate::ongoing::Ongoing;
use crate::outcome::TaskOutcome;
use crate::write::{PlaylistWriteOp, WriteError};

/// 投递给 worker 的一次写操作。
pub(crate) struct Job {
    /// 任务 id,worker 完成后用于从 [`Ongoing`] 移除。
    pub id: TaskId,

    /// 写操作载荷。
    pub op: PlaylistWriteOp,

    /// 取消令牌(只在开跑前生效,见模块文档)。
    pub cancel: CancellationToken,

    /// 终态通知通道(写一次)。
    pub done_tx: oneshot::Sender<TaskOutcome>,
}

/// PlaylistWrite lane:对外只暴露 [`PlaylistWriteLane::dispatch`]。
pub(crate) struct PlaylistWriteLane {
    /// 每个 source 一个发送端;`dispatch` 时按 [`SourceKind`] 路由。
    senders: FxHashMap<SourceKind, mpsc::UnboundedSender<Job>>,
}

impl PlaylistWriteLane {
    /// 启动 lane:为每个 channel spawn **一个** worker(串行保证的来源)。
    pub fn spawn(
        channels: &[Arc<dyn MusicChannel>],
        ongoing: &Arc<Ongoing>,
        event_tx: &Arc<Mutex<Vec<TaskEvent>>>,
    ) -> Self {
        let mut senders = FxHashMap::<SourceKind, mpsc::UnboundedSender<Job>>::default();
        for ch in channels {
            let source = ch.source();
            if senders.contains_key(&source) {
                continue;
            }
            let (tx, rx) = mpsc::unbounded_channel::<Job>();
            let channel = Arc::clone(ch);
            let ongoing = Arc::clone(ongoing);
            let event_tx = Arc::clone(event_tx);
            tokio::spawn(async move {
                worker_loop(channel, rx, ongoing, event_tx).await;
            });
            senders.insert(source, tx);
        }
        Self { senders }
    }

    /// 把一个 [`Job`] 投递到对应 source 的串行队列。
    /// channel 没注册时,job 直接以失败事件 + `Failed` 结束。
    pub fn dispatch(&self, source: SourceKind, job: Job, event_tx: &Arc<Mutex<Vec<TaskEvent>>>) {
        let Some(tx) = self.senders.get(&source) else {
            mineral_log::warn!(target: "playlist_write", ?source, "no channel registered");
            event_tx.lock().push(TaskEvent::PlaylistWriteDone {
                op: job.op,
                error: Some(WriteError::NotSupported),
            });
            let _ = job.done_tx.send(TaskOutcome::Failed);
            return;
        };
        let _ = tx.send(job);
    }
}

/// 单 worker 主循环:逐个跑完队列里的写操作,完成后从 ongoing 摘掉。
async fn worker_loop(
    channel: Arc<dyn MusicChannel>,
    mut rx: mpsc::UnboundedReceiver<Job>,
    ongoing: Arc<Ongoing>,
    event_tx: Arc<Mutex<Vec<TaskEvent>>>,
) {
    while let Some(job) = rx.recv().await {
        let id = job.id;
        mineral_log::debug!(target: "playlist_write", task_id = ?id, op = ?job.op, "worker start write");
        run_job(&channel, job, &event_tx).await;
        ongoing.remove(id);
    }
}

/// 执行一个写操作:排队期被取消则 `Cancelled`,否则一旦开跑必有终态事件。
async fn run_job(
    channel: &Arc<dyn MusicChannel>,
    job: Job,
    event_tx: &Arc<Mutex<Vec<TaskEvent>>>,
) {
    let Job {
        id: _,
        op,
        cancel,
        done_tx,
    } = job;
    if cancel.is_cancelled() {
        let _ = done_tx.send(TaskOutcome::Cancelled);
        return;
    }
    let outcome = execute(channel, op, event_tx).await;
    let _ = done_tx.send(outcome);
}

/// 调 channel 写方法;**成功失败都发 [`TaskEvent::PlaylistWriteDone`]**
/// (写操作的失败必须到达用户,不能只留日志)。
async fn execute(
    channel: &Arc<dyn MusicChannel>,
    op: PlaylistWriteOp,
    event_tx: &Arc<Mutex<Vec<TaskEvent>>>,
) -> TaskOutcome {
    let result = match &op {
        // create 的返回值(新歌单)刻意丢弃:数据收敛统一走"写成功 → 重拉
        // my_playlists"单一路径,不在这里旁路塞数据
        PlaylistWriteOp::Create { name, .. } => {
            channel.create_playlist(name).await.map(|_created| ())
        }
        PlaylistWriteOp::Delete { id } => channel.delete_playlist(id).await,
        PlaylistWriteOp::AddSongs { id, songs } => channel.playlist_add_songs(id, songs).await,
        PlaylistWriteOp::RemoveSongs { id, songs } => {
            channel.playlist_remove_songs(id, songs).await
        }
        PlaylistWriteOp::Rename { id, name } => channel.rename_playlist(id, name).await,
        PlaylistWriteOp::SetDescription { id, desc } => {
            channel.set_playlist_description(id, desc).await
        }
    };
    match result {
        Ok(()) => {
            event_tx.lock().push(TaskEvent::PlaylistWriteDone { op, error: None });
            TaskOutcome::Ok
        }
        Err(e) => {
            mineral_log::warn!(
                target: "playlist_write",
                ?op,
                error = mineral_log::chain(&e),
                "playlist write failed"
            );
            event_tx.lock().push(TaskEvent::PlaylistWriteDone {
                op,
                error: Some(WriteError::from_channel(&e)),
            });
            TaskOutcome::Failed
        }
    }
}
