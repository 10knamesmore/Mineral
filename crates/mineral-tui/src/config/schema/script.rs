//! 当前 TUI 的 Lua 执行预算;不包含音乐 hook 配置。

use mineral_config_macros::config_section;

/// TUI setup 与复制模板的执行预算
#[config_section]
#[derive(Copy)]
pub struct TuiScriptConfig {
    /// 每隔多少指令检查超时
    watchdog_instruction_interval: u32,

    /// 回调告警阈值，毫秒;超时继续执行
    watchdog_soft_wall_ms: u64,

    /// 回调中断阈值，毫秒;超时终止本次调用
    watchdog_hard_wall_ms: u64,
}

impl From<&TuiScriptConfig> for mineral_script::WatchdogConfig {
    /// 把本地配置的毫秒预算换算成执行参数;不在 Rust 中提供默认值。
    fn from(config: &TuiScriptConfig) -> Self {
        Self::builder()
            .instruction_interval(*config.watchdog_instruction_interval())
            .soft_wall(std::time::Duration::from_millis(
                *config.watchdog_soft_wall_ms(),
            ))
            .hard_wall(std::time::Duration::from_millis(
                *config.watchdog_hard_wall_ms(),
            ))
            .build()
    }
}
