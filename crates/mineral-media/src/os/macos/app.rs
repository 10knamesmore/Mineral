//! 主线程 NSApplication 与 run loop 驱动。
//!
//! 未打包二进制要被系统媒体中心收录、并接收媒体命令,必须在进程主线程养起一个
//! NSApplication 并持续 pump 其 run loop —— 命令只在主 run loop 上派发。activation
//! policy 设为 `Prohibited`:无 Dock 图标 / 菜单栏,纯后台。
//!
//! 注:Control Center 里点 app 图标会让系统重新 `open` 本可执行文件(又起一个进程),
//! 这是未打包 CLI 的固有限制——进程没注册进 LaunchServices,系统无法"激活已在跑的它",
//! 只能按路径重开。换 `Accessory` 也无效(实测仍重开)。要消除得打成 .app bundle,不做。
//!
//! 本模块经 objc2 绑定调 Objective-C,`unsafe` 仅用于读取框架的 run loop mode 常量。

#![allow(unsafe_code)]

use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::{MainThreadMarker, NSDate, NSDefaultRunLoopMode, NSRunLoop};

/// 主线程 NSApplication 句柄。`!Send`:只能在创建它的主线程上使用。
pub struct MacApp {
    /// 共享 NSApplication 实例,持有以保证 app 生命周期覆盖整个 daemon。
    _app: Retained<NSApplication>,
}

/// 每次 pump run loop 的时间片(秒):足够及时响应命令,又不忙等空转。
const PUMP_SLICE_SECS: f64 = 0.05;

/// 在主线程初始化 NSApplication(后台 accessory 形态),返回句柄。
///
/// 必须在进程主线程调用 —— 拿不到 [`MainThreadMarker`]说明不在主线程,返回 `Err`。
///
/// # Return:
///   就绪的 [`MacApp`];非主线程调用返回 `Err`。
pub fn macos_init_app() -> color_eyre::Result<MacApp> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err(color_eyre::eyre::eyre!(
            "macOS 系统媒体集成必须在进程主线程初始化 NSApplication"
        ));
    };
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    app.finishLaunching();
    Ok(MacApp { _app: app })
}

/// 在主线程阻塞 pump run loop,直到 `should_stop()` 为真。
///
/// 按 [`PUMP_SLICE_SECS`] 切片运行 run loop:每片让系统派发媒体命令 block,片末
/// 检查停止条件。无更多事件时 `runMode_beforeDate` 会睡到 limit,不忙等。
///
/// # Params:
///   - `app`: 主线程 NSApplication 句柄(确保 run loop 已就绪)。
///   - `should_stop`: 返回 `true` 时退出 pump。
pub fn macos_pump_until(app: &MacApp, should_stop: impl Fn() -> bool) {
    let _ = app;
    let run_loop = NSRunLoop::currentRunLoop();
    let mode = unsafe { NSDefaultRunLoopMode };
    while !should_stop() {
        let limit = NSDate::dateWithTimeIntervalSinceNow(PUMP_SLICE_SECS);
        run_loop.runMode_beforeDate(mode, &limit);
    }
}
