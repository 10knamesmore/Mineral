//! 歌词面板的跟唱渐变、行距与滚动手感。

use mineral_config_macros::config_section;

/// 歌词配置。
#[config_section]
pub struct LyricsConfig {
    /// 字词开始唱时渐入强调色的时长（毫秒）；0 为立即切换
    attack_ms: u64,

    /// 字词唱完后退为已唱正文色的时长（毫秒）；各词独立，0 为立即切换
    release_ms: u64,

    /// 歌词原文的明暗层级
    text_alpha: LyricTextAlphaConfig,

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

/// 歌词原文不透明度，范围 0–1；已唱用正文色，正在唱用强调色。
#[config_section]
pub struct LyricTextAlphaConfig {
    /// 当前行尚未唱到的部分
    unsung: f32,

    /// 当前行上下的邻行
    neighbor: f32,

    /// 可见范围最远处的歌词
    distant: f32,
}
