//! 顶层入口:[`Scheduler`]。

use std::sync::Arc;

use mineral_channel_core::MusicChannel;
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::event::TaskEvent;
use crate::handle::TaskHandle;
use crate::id::{Priority, TaskId};
use crate::kind::TaskKind;
use crate::lane::Lane;
use crate::lanes::channel_fetch::{ChannelFetchLane, Job as ChannelFetchJob};
use crate::lanes::cover_art::{CoverArtLane, Job as CoverArtJob};
use crate::ongoing::{Bind, Ongoing};

/// 进程内任务调度器入口。`Clone` 廉价(`Arc<Inner>`)。
#[derive(Clone)]
pub struct Scheduler {
    inner: Arc<Inner>,
}

struct Inner {
    ongoing: Arc<Ongoing>,
    events: Arc<Mutex<Vec<TaskEvent>>>,
    channel_fetch: ChannelFetchLane,
    cover_art: CoverArtLane,
}

/// `Scheduler::snapshot` 的返回:当前 running 数与按 lane 的拆分。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// 全部 lane 的 running 任务总数。
    pub running: usize,

    /// 每个 lane 的 running 任务计数(无任务的 lane 不出现)。
    pub by_lane: FxHashMap<Lane, usize>,
}

impl Scheduler {
    /// 用一组已构造好的 channel 初始化 scheduler。每个 channel 起对应 lane 的 worker 池。
    ///
    /// # Params:
    ///   - `channels`: 注入的所有 channel(用于 ChannelFetch lane 路由)
    pub fn new(channels: &[Arc<dyn MusicChannel>]) -> Self {
        let ongoing = Arc::new(Ongoing::new());
        let events = Arc::new(Mutex::new(Vec::<TaskEvent>::new()));
        let channel_fetch = ChannelFetchLane::spawn(channels, &ongoing, &events);
        let cover_art = CoverArtLane::spawn(&ongoing, &events);
        Self {
            inner: Arc::new(Inner {
                ongoing,
                events,
                channel_fetch,
                cover_art,
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
        match self.inner.ongoing.bind(kind.clone(), priority) {
            Bind::Shared(handle) => handle,
            Bind::Fresh {
                id,
                handle,
                done_tx,
            } => {
                self.dispatch(id, kind, priority, &handle, done_tx);
                handle
            }
        }
    }

    fn dispatch(
        &self,
        id: TaskId,
        kind: TaskKind,
        priority: Priority,
        handle: &TaskHandle,
        done_tx: tokio::sync::oneshot::Sender<crate::outcome::TaskOutcome>,
    ) {
        match kind {
            TaskKind::ChannelFetch(k) => {
                let source = k.source();
                self.inner.channel_fetch.dispatch(
                    source,
                    priority,
                    ChannelFetchJob {
                        id,
                        kind: k,
                        cancel: handle.cancel.clone(),
                        done_tx,
                    },
                );
            }
            TaskKind::CoverArt { url } => {
                self.inner.cover_art.dispatch(CoverArtJob {
                    id,
                    url,
                    cancel: handle.cancel.clone(),
                    done_tx,
                });
            }
        }
    }

    /// 单个取消。命中既存任务时 cancel 其 token;worker 在下个 await 点感知。
    pub fn cancel(&self, id: TaskId) {
        self.inner.ongoing.cancel(id);
    }

    /// 批量取消满足谓词的任务。常用于"输入又变了,砍掉所有 Search"等场景。
    pub fn cancel_where<F>(&self, pred: F)
    where
        F: Fn(&TaskKind) -> bool + Send + Sync,
    {
        self.inner.ongoing.cancel_where(&pred);
    }

    /// 从中央事件 buffer 拿走全部已积攒的事件,buffer 清空。UI 主循环 tick 时调一次。
    pub fn drain_events(&self) -> Vec<TaskEvent> {
        std::mem::take(&mut *self.inner.events.lock())
    }

    /// 当前调度状态快照。
    pub fn snapshot(&self) -> Snapshot {
        let counts = self.inner.ongoing.snapshot();
        Snapshot {
            running: counts.running,
            by_lane: counts.by_lane,
        }
    }
}
