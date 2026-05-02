//! Mineral 异步任务调度库。
//!
//! 进程内库,使用方 `Scheduler::new(channels)` 拿到一个 handle,在自己进程里
//! 提交 / 取消 / 拉事件。库本身不知道使用方是 TUI 还是 daemon。
//!
//! 入口:[`Scheduler`]。

mod event;
mod handle;
mod id;
mod kind;
mod lane;
mod lanes;
mod ongoing;
mod outcome;
mod scheduler;

pub use event::TaskEvent;
pub use handle::TaskHandle;
pub use id::{Priority, TaskId};
pub use kind::{ChannelFetchKind, DedupKey, TaskKind};
pub use lane::Lane;
pub use outcome::TaskOutcome;
pub use scheduler::{Scheduler, Snapshot};
