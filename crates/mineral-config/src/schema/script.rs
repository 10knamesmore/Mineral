//! 脚本运行时段(watchdog 双阈值)。
//!
//! daemon 进程在 VM 移交脚本线程前,据本段构造看门狗参数;非 daemon
//! 进程(no-op stub)不消费本段。

use mineral_config_macros::config_section;

/// 脚本运行
#[config_section]
#[derive(Copy)]
pub struct ScriptConfig {
    /// 每隔多少指令检查超时
    watchdog_instruction_interval: u32,

    /// 回调告警阈值，超时继续执行
    watchdog_soft_wall_ms: u64,

    /// 回调中断阈值，超时终止本次调用
    watchdog_hard_wall_ms: u64,

    /// 拦截回调等待上限，超时放行
    hook_timeout_ms: u64,

    /// 子进程并发上限；0 为不限
    spawn_max_concurrent: usize,
}
