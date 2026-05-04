//! Mineral 后台 server 单进程骨架。
//!
//! 把 audio engine、task scheduler、channels 等「server 角色」对象收成 [`Server`],
//! 对外发 [`ClientHandle`] 作为 client 的指令面。当前实现是同进程 ——
//! 所有方法都只是把调用透传给底层 [`mineral_audio::AudioHandle`] /
//! [`mineral_task::Scheduler`]。未来真要拆双进程时,只换 [`ClientHandle`] 内部
//! 实现(改成 unix socket / serde 编码),签名保持稳定,client 调用方零改动。
//!
//! ## 角色边界
//!
//! - **Server**:拥有 audio engine 线程、task worker、channels;在 [`Server::spawn`]
//!   时启动,在被 drop 时清理。提供 [`Server::client`] 发 cheap-clone handle、
//!   [`Server::take_spectrum_tap`] 拿走唯一的 PCM 旁路 consumer、[`Server::shutdown`]
//!   显式终止。
//! - **ClientHandle**:`Clone`,只暴露「调命令 + 拉 snapshot + 拉事件」。**不要**
//!   在签名里漏出 `&AudioHandle` / `&Scheduler` 这种内部类型,否则将来换 IPC 时
//!   破坏调用方。

mod client;
mod serve;
mod server;

pub use client::ClientHandle;
pub use mineral_protocol::{CancelFilter, ChannelFetchKindTag};
pub use server::Server;
