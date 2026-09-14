//! TUI 后端端口:生产实现是 [`mineral_client::Client`] 会话,测试实现是进程内假后端。
//!
//! 端口只做两件事:
//! - **读镜像**:共享状态从本地订阅镜像读取,绘制与输入处理无需等待 IPC 应答。
//! - **提交操作**:操作立即入队并返回;结论经 [`CompletionQueue`] 异步回流,
//!   App 每帧 drain,失败按结构化类别提示。
//!
//! 这是 UI 与 client 的接缝(测试替身也走同一接口),不是 daemon 侧的业务契约:
//! 生产路径唯一实现是 [`ClientBackend`]。

mod client;
mod completion;
mod port;
mod task_submissions;

#[cfg(test)]
mod tests;

pub(crate) use client::ClientBackend;
pub(crate) use completion::{Completion, CompletionQueue};
pub(crate) use port::{Backend, BackendBootstrap};
