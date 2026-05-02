//! 进行中任务的中央状态聚合。

use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use tokio_util::sync::CancellationToken;

use crate::handle::{SharedDone, TaskHandle, shared_done};
use crate::id::{IdAllocator, Priority, TaskId};
use crate::kind::{DedupKey, TaskKind};
use crate::lane::Lane;
use crate::outcome::TaskOutcome;
use tokio::sync::oneshot;

/// 已记录在 ongoing 中的一条任务。
pub(crate) struct TaskMeta {
    pub kind: TaskKind,
    pub priority: Priority,
    pub cancel: CancellationToken,
    pub done: SharedDone,
}

impl TaskMeta {
    fn handle(&self, id: TaskId) -> TaskHandle {
        TaskHandle {
            id,
            cancel: self.cancel.clone(),
            done: self.done.clone(),
        }
    }
}

/// 中央状态。所有 mutate 操作都在持锁里完成。
pub(crate) struct Ongoing {
    inner: Mutex<Inner>,
    ids: IdAllocator,
}

struct Inner {
    tasks: FxHashMap<TaskId, TaskMeta>,
    by_dedup: FxHashMap<DedupKey, TaskId>,
}

/// 一次新提交的"绑定结果":要么 dedup 共享旧 handle,要么真新建一条。
pub(crate) enum Bind {
    /// dedup 命中既存任务,直接复用其 handle。
    Shared(TaskHandle),

    /// 新登记一条任务,调用方拿到 handle 与 done sender(供 worker 上报终态)。
    Fresh {
        id: TaskId,
        handle: TaskHandle,
        done_tx: oneshot::Sender<TaskOutcome>,
    },
}

impl Ongoing {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                tasks: FxHashMap::default(),
                by_dedup: FxHashMap::default(),
            }),
            ids: IdAllocator::default(),
        }
    }

    /// 提交时绑定:dedup + escalate 决策。
    ///
    /// 行为:
    /// - dedup 命中 + 新 prio ≤ 旧 prio → `Shared(旧 handle)`,什么都不变。
    /// - dedup 命中 + 新 prio > 旧 prio → cancel 旧任务、移除旧条目、新建条目。
    /// - dedup 不命中 → 新建条目。
    pub fn bind(&self, kind: TaskKind, priority: Priority) -> Bind {
        let dedup = kind.dedup_key();
        let mut inner = self.inner.lock();
        if let Some(existing_id) = inner.by_dedup.get(&dedup).copied() {
            if let Some(existing) = inner.tasks.get(&existing_id) {
                if priority <= existing.priority {
                    return Bind::Shared(existing.handle(existing_id));
                }
                // escalate:cancel 旧、走下面的新建路径。
                existing.cancel.cancel();
            }
            inner.tasks.remove(&existing_id);
            inner.by_dedup.remove(&dedup);
        }
        let id = self.ids.next();
        let cancel = CancellationToken::new();
        let (done_tx, done_rx) = oneshot::channel();
        let done = shared_done(done_rx);
        let meta = TaskMeta {
            kind,
            priority,
            cancel,
            done,
        };
        let handle = meta.handle(id);
        inner.tasks.insert(id, meta);
        inner.by_dedup.insert(dedup, id);
        Bind::Fresh {
            id,
            handle,
            done_tx,
        }
    }

    /// worker 完成任务后从中央状态摘除。dedup 索引同步清理。
    pub fn remove(&self, id: TaskId) {
        let mut inner = self.inner.lock();
        if let Some(meta) = inner.tasks.remove(&id) {
            let key = meta.kind.dedup_key();
            // 仅当 dedup 仍指向被摘的 id 时才移除,避免误清新建条目。
            if inner.by_dedup.get(&key) == Some(&id) {
                inner.by_dedup.remove(&key);
            }
        }
    }

    /// 单个取消。命中就 cancel 对应 token。
    pub fn cancel(&self, id: TaskId) {
        let inner = self.inner.lock();
        if let Some(meta) = inner.tasks.get(&id) {
            meta.cancel.cancel();
        }
    }

    /// 批量取消满足谓词的任务。
    pub fn cancel_where(&self, pred: &(dyn Fn(&TaskKind) -> bool + Send + Sync)) {
        let inner = self.inner.lock();
        for meta in inner.tasks.values() {
            if pred(&meta.kind) {
                meta.cancel.cancel();
            }
        }
    }

    /// 当前 running 计数(含 enqueued 未真正开跑的)。
    pub fn snapshot(&self) -> SnapshotCounts {
        let inner = self.inner.lock();
        let mut by_lane = FxHashMap::<Lane, usize>::default();
        for meta in inner.tasks.values() {
            *by_lane.entry(meta.kind.lane()).or_insert(0) += 1;
        }
        SnapshotCounts {
            running: inner.tasks.len(),
            by_lane,
        }
    }
}

/// `Ongoing::snapshot` 的数据载荷。
pub(crate) struct SnapshotCounts {
    pub running: usize,
    pub by_lane: FxHashMap<Lane, usize>,
}
