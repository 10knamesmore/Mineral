//! toast 段(挂在 `TuiConfig` 下):通知停留时长。

use mineral_config_macros::config_section;

/// 通知
#[config_section]
pub struct ToastConfig {
    /// 临时通知停留时长
    flash_ttl_secs: u64,
}
