//! 任务身份与优先级。

use std::sync::atomic::{AtomicU64, Ordering};

/// 全进程唯一的任务 id。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(u64);

impl TaskId {
    /// 暴露内部数值,主要给日志 / debug 用。
    pub fn raw(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "T{}", self.0)
    }
}

/// 单调递增的 id 分配器。
#[derive(Debug, Default)]
pub(crate) struct IdAllocator(AtomicU64);

impl IdAllocator {
    pub(crate) fn next(&self) -> TaskId {
        TaskId(self.0.fetch_add(1, Ordering::Relaxed))
    }
}

/// 调度优先级。`User` 永远排在 `Background` 前面;`User` 之间 FIFO。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// 后台任务(预热、扫盘等)。
    Background,

    /// 用户即时操作产生的任务。
    User,
}
