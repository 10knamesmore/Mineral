//! 预取段(挂在 `TuiConfig` 下):各 lookahead 半径 + 去抖。

use mineral_config_macros::config_section;

/// 预取配置。
#[config_section]
pub struct PrefetchConfig {
    /// 选中行前后预取条数
    radius: usize,

    /// 在播曲前后预取封面数
    playback_cover_radius: usize,

    /// 后续封面预编码首数
    prewarm_ahead: usize,
}
