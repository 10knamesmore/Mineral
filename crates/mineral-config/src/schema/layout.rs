//! 布局段(挂在 `TuiConfig` 下):完整布局门槛 + 全屏分区尺寸 + 浮层 dock 宽。

use serde::Deserialize;

/// 布局配置。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[derive(Clone, Debug, Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LayoutConfig {
    /// 启用完整布局的最小终端宽(列);不足走紧凑布局。
    min_full_width: u16,

    /// 启用完整布局的最小终端高(行)。
    min_full_height: u16,

    /// 全屏态左栏占比(%)。
    fs_left_pct: u16,

    /// 全屏态频谱区高(行)。
    fs_spectrum_height: u16,

    /// 全屏态 transport 区高(行)。
    fs_transport_height: u16,

    /// 浮层 dock 宽占比(%)。
    dock_w_pct: u16,
}
