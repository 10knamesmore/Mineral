//! Mineral 顶层 CLI 分发。
//!
//! 模块拆分（均为私有，仅通过下面的 `pub use` 暴露入口）：
//! - `core` —— 顶层 [`Args`] / [`Command`] 与 [`run`] 入口
//! - `subcommands` —— 各 namespace 的子命令树（目前只有 `subcommands::channel`）
//!
//! 业务实现都不放本 crate；channel 自身的 CLI 在
//! [`mineral_channel_netease::cli`] 这类 channel 自己的 crate 里。

mod core;
mod subcommands;

pub use crate::core::{run, Args, Command};
