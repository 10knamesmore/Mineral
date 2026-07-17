//! toast 段(挂在 `TuiConfig` 下):通知停留时长。

use mineral_config_macros::config_section;

/// toast 配置。
#[config_section]
pub struct ToastConfig {
    /// 一次性通知(下载完成 / 配置告警等)toast 停留时长(秒)。
    flash_ttl_secs: u64,
}
