//! 任务句柄。

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use tokio::sync::oneshot;

use crate::outcome::TaskOutcome;

/// `Shared` 包装的"等任务终态"future,可被多个 waiter 同时 await。
pub(crate) type SharedDone = Shared<BoxFuture<'static, TaskOutcome>>;

/// 把 oneshot Receiver 包装成 Shared,Sender 被 drop 时自动映射成 [`TaskOutcome::Cancelled`]。
pub(crate) fn shared_done(rx: oneshot::Receiver<TaskOutcome>) -> SharedDone {
    let fut: BoxFuture<'static, TaskOutcome> =
        async move { rx.await.unwrap_or(TaskOutcome::Cancelled) }.boxed();
    fut.shared()
}

/// 提交任务后拿到的终态句柄。
///
/// `Clone` 等价于"多人持有同一任务"——dedup 命中时 [`crate::Scheduler::submit`]
/// 会返回原任务的 handle 副本。
#[derive(Clone)]
pub struct TaskHandle {
    /// 终态 future,多个 waiter 可同时 await 拿到同一份 [`TaskOutcome`]。
    pub(crate) done: SharedDone,
}

impl TaskHandle {
    /// 等待终态。可被多次 / 多个 waiter 调用,会拿到相同的 [`TaskOutcome`]。
    pub async fn done(&self) -> TaskOutcome {
        self.done.clone().await
    }
}
