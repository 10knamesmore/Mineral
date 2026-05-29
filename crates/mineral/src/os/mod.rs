//! 平台相关的 daemon 入口分发:`mineral serve` 的启动方式按 `target_os` 选择。
//!
//! - macOS:主线程让给 AppKit,tokio 挪后台线程(系统媒体集成需要主线程 run loop)。
//! - 其它平台:主线程直接 block_on tokio。

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub(crate) use macos::run_daemon;

#[cfg(not(target_os = "macos"))]
mod linux;

#[cfg(not(target_os = "macos"))]
pub(crate) use linux::run_daemon;
