//! toast 段(挂在 `TuiConfig` 下):通知停留时长。

use mineral_config_macros::config_section;

/// toast 配置。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
pub struct ToastConfig {
    /// 通知 toast 停留时长(秒)。
    flash_ttl_secs: u64,
}
