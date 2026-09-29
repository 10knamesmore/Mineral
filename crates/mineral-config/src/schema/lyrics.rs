//! 歌词段(挂在 `TuiConfig` 下):全屏沉浸态行距 + 滚动手感。

use mineral_config_macros::config_section;

/// 歌词配置。
#[config_section]
pub struct LyricsConfig {
    /// 全屏歌词空行数；0 时滚动直接跳转
    fullscreen_line_gap: usize,

    /// 紧凑歌词空行数
    compact_line_gap: usize,

    /// 歌词切行过渡时长
    scroll_ms: u64,

    /// 滚动后恢复跟唱的等待时长
    reattach_ms: u32,

    /// 边界回弹阻尼；0 按 1 处理
    overshoot_damping: u32,

    /// 回弹上限（千分之一行）；0 为关闭
    overshoot_max_permille: u32,
}
