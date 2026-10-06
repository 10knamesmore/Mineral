//! daemon 的 Lua 执行预算与音乐 hook 等待上限。

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
}
