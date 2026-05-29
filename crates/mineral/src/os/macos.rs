//! macOS daemon 入口:主线程让给 AppKit(NSApplication + run loop),tokio 挪后台线程。
//!
//! 系统媒体中心只把命令派发到主线程 run loop,未打包二进制也要主线程养起 NSApplication
//! 才会被收录。故主线程初始化 NSApplication 后 pump run loop,真正的 daemon 跑在后台线程;
//! 后台线程结束(优雅收尾或启动失败)即置 `done`,主线程退出 pump 并冒泡其结果。

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use color_eyre::eyre::WrapErr;

/// daemon 入口(`mineral serve`,macOS)。
pub(crate) fn run_daemon() -> color_eyre::Result<()> {
    let app = mineral_media::macos_init_app()?;
    let done = Arc::new(AtomicBool::new(false));
    let outcome = Arc::new(Mutex::new(Option::<color_eyre::Result<()>>::None));
    let worker = std::thread::Builder::new()
        .name("mineral-daemon-host".to_owned())
        .spawn({
            let done = Arc::clone(&done);
            let outcome = Arc::clone(&outcome);
            move || {
                let result = crate::serve_blocking();
                if let Ok(mut slot) = outcome.lock() {
                    *slot = Some(result);
                }
                done.store(true, Ordering::SeqCst);
            }
        })
        .wrap_err("spawn daemon host thread")?;
    mineral_media::macos_pump_until(&app, || done.load(Ordering::SeqCst));
    let _ = worker.join();
    let result = outcome.lock().ok().and_then(|mut slot| slot.take());
    result.unwrap_or(Ok(()))
}
