//! 列表右边框 minimap 段(挂在 `TuiConfig` 下)。

use mineral_config_macros::config_section;

/// 列表右边框 minimap 配置。
#[config_section]
pub struct MinimapConfig {
    /// 最小光晕半径(轨道行数)
    halo_rows: u64,

    /// 吸附区半径
    magnet_dots: u64,
}
