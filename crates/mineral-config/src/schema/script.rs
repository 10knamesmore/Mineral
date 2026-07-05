//! 脚本运行时段(watchdog 双阈值)。
//!
//! daemon 进程在 VM 移交脚本线程前,据本段构造看门狗参数;非 daemon
//! 进程(no-op stub)不消费本段。

use mineral_config_macros::config_section;

/// 脚本运行时段。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
#[derive(Copy)]
pub struct ScriptConfig {
    /// 看门狗:每多少条 Lua VM 指令检查一次墙钟(越小越灵敏、开销越大)。
    watchdog_instruction_interval: u32,

    /// 看门狗软阈值(毫秒):回调超过记一次 warn 日志,继续执行。
    watchdog_soft_wall_ms: u64,

    /// 看门狗硬阈值(毫秒):回调超过被中断(只杀本次调用,VM 保留)。
    watchdog_hard_wall_ms: u64,

    /// 同步拦截 hook(`before_stream` / `before_download`)软超时(毫秒):
    /// 超时未回执按放行处理 + warn,播放 / 下载不被慢 hook 卡住。
    hook_timeout_ms: u64,

    /// `mineral.spawn` 子进程并发上限(防脚本 fork 炸);0 = 不限。
    spawn_max_concurrent: usize,
}
