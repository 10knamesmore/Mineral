//! toast 段(挂在 `TuiConfig` 下):通知停留时长。

use serde::Deserialize;

/// toast 配置。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[derive(Clone, Debug, Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ToastConfig {
    /// 通知 toast 停留时长(秒)。
    flash_ttl_secs: u64,
}
