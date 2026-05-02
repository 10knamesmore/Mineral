//! 任务句柄。

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::id::TaskId;
use crate::outcome::TaskOutcome;

/// `Shared` 包装的"等任务终态"future,可被多个 waiter 同时 await。
pub(crate) type SharedDone = Shared<BoxFuture<'static, TaskOutcome>>;

/// 把 oneshot Receiver 包装成 Shared,Sender 被 drop 时自动映射成 [`TaskOutcome::Cancelled`]。
pub(crate) fn shared_done(rx: oneshot::Receiver<TaskOutcome>) -> SharedDone {
    let fut: BoxFuture<'static, TaskOutcome> =
        async move { rx.await.unwrap_or(TaskOutcome::Cancelled) }.boxed();
    fut.shared()
}

/// 提交任务后拿到的句柄:可取消,可 await 等终态。
///
/// `Clone` 等价于"多人持有同一任务"——dedup 命中时 [`crate::Scheduler::submit`]
/// 会返回原任务的 handle 副本。
#[derive(Clone)]
pub struct TaskHandle {
    /// 任务 id。
    pub id: TaskId,

    pub(crate) cancel: CancellationToken,
    pub(crate) done: SharedDone,
}

impl TaskHandle {
    /// 请求取消。任务可能已经在跑或还在队列里;取消是协作式的,worker 在
    /// 下一个 await 点检测到。
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    /// 等待终态。可被多次 / 多个 waiter 调用,会拿到相同的 [`TaskOutcome`]。
    pub async fn done(&self) -> TaskOutcome {
        self.done.clone().await
    }
}
