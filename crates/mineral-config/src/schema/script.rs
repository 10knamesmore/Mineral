//! 脚本运行时段(watchdog 双阈值)。
//!
//! daemon 进程在 VM 移交脚本线程前,据本段构造看门狗参数;非 daemon
//! 进程(no-op stub)不消费本段。

use serde::Deserialize;

/// 脚本运行时段。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[derive(Clone, Copy, Debug, Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ScriptConfig {
    /// 看门狗:每多少条 Lua VM 指令检查一次墙钟(越小越灵敏、开销越大)。
    watchdog_instruction_interval: u32,

    /// 看门狗软阈值(毫秒):回调超过记一次 warn 日志,继续执行。
    watchdog_soft_wall_ms: u64,

    /// 看门狗硬阈值(毫秒):回调超过被中断(只杀本次调用,VM 保留)。
    watchdog_hard_wall_ms: u64,
}
