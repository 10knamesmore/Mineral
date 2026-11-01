//! 顶层入口:[`Scheduler`]。

use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use async_stream::try_stream;
use futures_util::StreamExt;
use mineral_channel_core::store::{LibraryStore, NamespaceStore};
use mineral_channel_core::{
    Error as ChannelError, FetchSnapshot, FetchStream, MusicChannel, PlaylistDetail,
};
use mineral_model::{PlaylistId, RefreshScope};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ChannelFetchKind;
use crate::TaskOutcome;
use crate::event::TaskEvent;
use crate::handle::TaskHandle;
use crate::id::{Priority, TaskId};
use crate::kind::ChannelFetchKindTag;
use crate::kind::TaskKind;
use crate::lanes::channel_fetch::{ChannelFetchLane, DispatchError, Job as ChannelFetchJob};
use crate::lanes::playlist_write::{Job as PlaylistWriteJob, PlaylistWriteLane};
use crate::ongoing::{Bind, Ongoing};
use crate::read_cache::ReadCache;

/// A queued operation either failed to execute, failed at its source, or was cancelled.
#[derive(Debug, Error)]
pub enum ChannelRunError<E> {
    /// The source queue could not grant an execution turn.
    #[error(transparent)]
    Dispatch(#[from] DispatchError),

    /// The source returned an error.
    #[error("channel operation: {0}")]
    Source(#[source] E),

    /// The existing task cancellation mechanism cancelled this operation.
    #[error("channel operation cancelled")]
    Cancelled,
}

/// Accounts for one active operation; no entry is retained while its stream is paused.
struct ActiveOperation {
    /// Existing task accounting and cancellation owner.
    ongoing: Arc<Ongoing>,

    /// Existing task event queue, used only for the completion metric.
    events: Arc<Mutex<Vec<TaskEvent>>>,

    /// Task identity for this active operation, released when the operation ends.
    id: TaskId,

    /// Source and target recorded by the task system.
    kind: ChannelFetchKind,

    /// Priority chosen by the requesting consumer.
    priority: Priority,

    /// Includes queue wait and execution of this operation.
    started: Instant,

    /// Remains Cancelled when the consumer drops its future.
    outcome: TaskOutcome,
}

impl Drop for ActiveOperation {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed();
        let latency_ms = elapsed
            .as_secs()
            .saturating_mul(1000)
            .saturating_add(u64::from(elapsed.subsec_millis()));
        self.events.lock().push(TaskEvent::FetchDone {
            kind: ChannelFetchKindTag::of(&self.kind),
            source: self.kind.source(),
            target_ref: self.kind.target_ref(),
            from_user: self.priority == Priority::User,
            outcome: self.outcome,
            latency_ms,
        });
        self.ongoing.remove(self.id);
    }
}

/// 进程内任务调度器入口。`Clone` 廉价(`Arc<Inner>`)。
#[derive(Clone)]
pub struct Scheduler {
    /// 共享内部状态(中央 ongoing / 事件 buffer / 各 lane)。
    inner: Arc<Inner>,
}

/// Scheduler 的真实状态,被 `Arc` 包起来共享。
struct Inner {
    /// Source caches become reusable only after a complete fresh read.
    read_cache: Arc<ReadCache>,

    /// 中央 ongoing 状态(任务表 + dedup 索引)。
    ongoing: Arc<Ongoing>,

    /// 中央事件 buffer(纯 [`TaskEvent`] 事实,可 serde):lane worker 推、消费方
    /// [`Scheduler::drain_events`] 拉。
    events: Arc<Mutex<Vec<TaskEvent>>>,

    /// ChannelFetch lane(per-channel worker 池)。
    channel_fetch: ChannelFetchLane,

    /// PlaylistWrite lane(per-source 单 worker 串行)。
    playlist_write: PlaylistWriteLane,
}

/// `Scheduler::snapshot` 的返回:当前 running 数与按 fetch kind 的拆分。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// 全部 running 任务总数。
    pub running: usize,

    /// `ChannelFetch` 任务按 [`ChannelFetchKindTag`] 细分(其它种类不在此 map)。
    /// UI 用它显示 `pl:1 tr:2 song:1 lyr:0 ♥:0` 级别的拆分。
    pub by_kind: FxHashMap<ChannelFetchKindTag, usize>,
}

impl Scheduler {
    /// Runs an operation in the source's priority queue. The caller owns its state between calls.
    /// Cancelling the returned future releases the execution turn.
    pub async fn run_channel<T, E>(
        &self,
        kind: ChannelFetchKind,
        priority: Priority,
        operation: impl Future<Output = Result<T, E>>,
    ) -> Result<T, ChannelRunError<E>> {
        let source = kind.source();
        let (id, cancel) = self
            .inner
            .ongoing
            .track(TaskKind::ChannelFetch(kind.clone()), priority);
        let mut active = ActiveOperation {
            ongoing: Arc::clone(&self.inner.ongoing),
            events: Arc::clone(&self.inner.events),
            id,
            kind,
            priority,
            started: Instant::now(),
            outcome: TaskOutcome::Cancelled,
        };
        let result = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(ChannelRunError::Cancelled),
            result = self.inner.channel_fetch.run(source, priority, operation) => result,
        };
        match result {
            Ok(Ok(value)) => {
                active.outcome = TaskOutcome::Ok;
                Ok(value)
            }
            Ok(Err(error)) => {
                active.outcome = TaskOutcome::Failed;
                Err(ChannelRunError::Source(error))
            }
            Err(error) => {
                active.outcome = TaskOutcome::Failed;
                Err(ChannelRunError::Dispatch(error))
            }
        }
    }

    /// Creates a lazy playlist stream using the current invalidation policy.
    pub fn playlist_stream(&self, id: PlaylistId) -> FetchStream<'static, PlaylistDetail> {
        let kind = ChannelFetchKind::PlaylistDetail { id: id.clone() };
        let cache = Arc::clone(&self.inner.read_cache);
        let (policy, revision) = cache.read_state(&kind);
        let mut snapshots = self.inner.channel_fetch.playlist(id, policy);
        Box::pin(try_stream! {
            loop {
                if cache.read_state(&kind).1 != revision {
                    Err(ChannelError::InvalidData { field: "playlist read invalidated" })?;
                }
                let Some(snapshot) = snapshots.next().await else {
                    break;
                };
                let snapshot = snapshot?;
                let complete = matches!(&snapshot, FetchSnapshot::Fresh(detail) if detail.complete);
                if !cache.accept_result(&kind, revision, complete) {
                    Err(ChannelError::InvalidData { field: "playlist read invalidated" })?;
                }
                yield snapshot;
            }
        })
    }

    /// 用一组已构造好的 channel 初始化 scheduler。每个 channel 起对应 lane 的 worker 池。
    ///
    /// # Params:
    ///   - `channels`: 注入的所有 channel(用于 ChannelFetch lane 路由)
    ///   - `workers_per_channel`: 每个 channel 的任务 worker 数(配置 `channel_workers_per`)
    ///   - `stores`: 按来源写回刷新后的歌曲资料，不改变收藏与本地歌单成员关系。
    ///   - `library`: 本地歌单身份与顺序；不提供时不能刷新聚合歌单。
    pub fn new(
        channels: &[Arc<dyn MusicChannel>],
        workers_per_channel: usize,
        stores: &[Arc<dyn NamespaceStore>],
        library: Option<Arc<dyn LibraryStore>>,
    ) -> Self {
        let ongoing = Arc::new(Ongoing::new());
        let events = Arc::new(Mutex::new(Vec::<TaskEvent>::new()));
        let channel_fetch = ChannelFetchLane::spawn(
            channels,
            &ongoing,
            &events,
            workers_per_channel,
            stores,
            library,
        );
        let playlist_write = PlaylistWriteLane::spawn(channels, &ongoing, &events);
        Self {
            inner: Arc::new(Inner {
                read_cache: Arc::new(ReadCache::default()),
                ongoing,
                events,
                channel_fetch,
                playlist_write,
            }),
        }
    }

    /// 提交一个任务。
    ///
    /// # 行为
    ///   - 若已存在同 `dedup_key` 的进行中任务且新优先级 ≤ 旧优先级,直接返回旧 handle 的副本。
    ///   - 若已存在但新优先级更高,旧任务被 cancel,新任务进对应优先级队列。
    ///   - 否则按种类路由到对应 lane。
    pub fn submit(&self, kind: TaskKind, priority: Priority) -> TaskHandle {
        let _publication =
            matches!(&kind, TaskKind::ChannelFetch(_)).then(|| self.inner.events.lock());
        match self.inner.ongoing.bind(kind.clone(), priority) {
            Bind::Shared(handle) => {
                mineral_log::debug!(target: "scheduler", kind = ?kind, "submit deduped to existing task");
                handle
            }
            Bind::Fresh {
                id,
                handle,
                cancel,
                done_tx,
            } => {
                mineral_log::debug!(target: "scheduler", task_id = ?id, kind = ?kind, priority = ?priority, "submit new task");
                self.dispatch(id, kind, priority, cancel, done_tx);
                handle
            }
        }
    }

    /// 把新建任务路由到对应的 source-specific lane。
    fn dispatch(
        &self,
        id: TaskId,
        kind: TaskKind,
        priority: Priority,
        cancel: tokio_util::sync::CancellationToken,
        done_tx: tokio::sync::oneshot::Sender<crate::outcome::TaskOutcome>,
    ) {
        match kind {
            TaskKind::ChannelFetch(k) => {
                let source = k.source();
                let cache = self.inner.read_cache.policy(&k);
                self.inner.channel_fetch.dispatch(
                    source,
                    priority,
                    ChannelFetchJob {
                        id,
                        kind: k,
                        priority,
                        cancel,
                        done_tx,
                        cache,
                        read_cache: Arc::clone(&self.inner.read_cache),
                    },
                );
            }
            TaskKind::PlaylistWrite(op) => {
                let source = op.target_source();
                self.inner.playlist_write.dispatch(
                    source,
                    PlaylistWriteJob {
                        id,
                        op,
                        cancel,
                        done_tx,
                    },
                    &self.inner.events,
                );
            }
        }
    }

    /// 取消满足谓词的内部任务；worker 在下个 await 点停止。
    pub fn cancel_where<F>(&self, pred: F)
    where
        F: Fn(&TaskKind) -> bool + Send + Sync,
    {
        self.inner.ongoing.cancel_where(&pred);
    }

    /// Cancellation, buffered-result removal and replacement binding share the publication lock.
    pub fn invalidate(&self, scope: &RefreshScope) {
        let mut events = self.inner.events.lock();
        self.inner.ongoing.invalidate(
            |kind| matches!(kind, TaskKind::ChannelFetch(fetch) if fetch.affected_by(scope)),
        );
        events.retain(|event| !event.affected_by(scope));
        self.inner.read_cache.invalidate(scope);
    }

    /// 从中央事件 buffer 拿走全部已积攒的事件(纯 [`TaskEvent`] 事实),buffer 清空。
    /// 消费方主循环 tick 时调一次。
    pub fn drain_events(&self) -> Vec<TaskEvent> {
        std::mem::take(&mut *self.inner.events.lock())
    }

    /// 当前调度状态快照。
    pub fn snapshot(&self) -> Snapshot {
        let counts = self.inner.ongoing.snapshot();
        Snapshot {
            running: counts.running,
            by_kind: counts.by_kind,
        }
    }
}
