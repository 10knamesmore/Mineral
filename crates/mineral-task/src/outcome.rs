//! 任务终态。

/// 任务终态。详细错误进 log,这里只暴露三态,方便 UI / 调用方判断要不要重试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskOutcome {
    /// 业务成功。
    Ok,

    /// 被 [`crate::Scheduler::cancel`] 取消(或被 escalate 替换)。
    Cancelled,

    /// 业务失败,具体错误已写入 `mineral-log`。
    Failed,
}
