//! macOS daemon 入口：主线程运行 AppKit，后台线程承载异步服务。
//!
//! 系统媒体命令依赖主线程 run loop。后台线程完成后停止 pump，并将服务错误或 panic
//! 传回主线程，避免后台异常退出后继续等待。

use crate::error::{Error, Result};

/// 初始化系统媒体应用并运行 daemon，直到服务退出。
pub(crate) fn run_daemon() -> Result<()> {
    let app = mineral_media::macos_init_app()?;
    let worker = std::thread::Builder::new()
        .name("mineral-daemon-host".to_owned())
        .spawn(crate::serve_blocking)
        .map_err(Error::DaemonThread)?;
    mineral_media::macos_pump_until(&app, || worker.is_finished());
    match worker.join() {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}
