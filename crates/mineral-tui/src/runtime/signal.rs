//! TUI 进程的退出信号:SIGTERM / SIGINT / SIGHUP。
//!
//! TUI 跑在 raw mode + alternate screen 下,这些信号若走默认处置会直接杀进程
//! (终端花屏 + 无日志,silent dead)。这里用 `tokio::signal`(与 daemon 端一致)
//! 起一个后台 task 监听:收到任一信号就记一条日志并置 shutdown 标志,主循环据此
//! 走正常退出路径(`Tui::exit` 还原终端)。
//!
//! 注:SIGKILL / SIGSTOP 不可捕获,仍会 silent dead —— 内核保留信号,无解。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use color_eyre::eyre::WrapErr;
use tokio::signal::unix::{SignalKind, signal};

/// 起后台 task 监听退出信号,返回 shutdown 标志(收到信号后变 `true`)。
///
/// 必须在 tokio runtime 上下文里调用(`signal` / `tokio::spawn` 都依赖它)。
/// 日志在 task 内打 —— 普通 async 代码,既安全又能直接带上信号名。
///
/// # Return:
///   主循环轮询用的标志:`false` = 未触发,`true` = 收到过退出信号。
pub(crate) fn spawn_watcher() -> color_eyre::Result<Arc<AtomicBool>> {
    let mut term = signal(SignalKind::terminate()).wrap_err("install SIGTERM handler")?;
    let mut interrupt = signal(SignalKind::interrupt()).wrap_err("install SIGINT handler")?;
    let mut hangup = signal(SignalKind::hangup()).wrap_err("install SIGHUP handler")?;

    let flag = Arc::new(AtomicBool::new(false));
    let watch = Arc::clone(&flag);
    tokio::spawn(async move {
        let name = tokio::select! {
            _ = term.recv() => "SIGTERM",
            _ = interrupt.recv() => "SIGINT",
            _ = hangup.recv() => "SIGHUP",
        };
        mineral_log::info!(target: "tui", signal = name, "received signal, shutting down");
        watch.store(true, Ordering::Release);
    });
    Ok(flag)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use color_eyre::eyre::eyre;
    use nix::sys::signal::{Signal, raise};

    use super::spawn_watcher;

    /// 起 watcher 后给自己发 SIGINT:`signal()` 已在 `spawn_watcher` 内同步注册,
    /// 默认处置被抑制 → raise 不会杀测试进程,而是被 watcher 捕获,把标志翻 `true`。
    #[tokio::test]
    async fn watcher_flips_flag_on_signal() -> color_eyre::Result<()> {
        let flag = spawn_watcher()?;
        assert!(!flag.load(Ordering::Acquire), "起始应为 false");

        raise(Signal::SIGINT).map_err(|e| eyre!("raise SIGINT: {e}"))?;

        // 给后台 task 一拍把信号转成标志(带超时,防挂死)。
        let deadline = Instant::now() + Duration::from_secs(5);
        while !flag.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "5s 内未观察到标志翻转");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }
}
