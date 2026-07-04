//! 歌词段(挂在 `TuiConfig` 下):全屏沉浸态行距 + 滚动手感。

use mineral_config_macros::config_section;

/// 歌词配置。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
pub struct LyricsConfig {
    /// 全屏沉浸态行与行之间垫的空行数(`0` = 紧凑)。
    fullscreen_line_gap: usize,

    /// 非全屏紧凑态行与行之间垫的空行数(`0` = 紧凑)。
    compact_line_gap: usize,

    /// 当前行切换后整列平移 + 高亮交叉淡入的过渡时长(毫秒)。
    scroll_ms: u64,

    /// 有时间戳歌手动滚走后,空闲多久(毫秒)自动平滑回到跟随当前行。无时间戳歌不回锚。
    reattach_ms: u32,

    /// 滚到头再滚时,画面会多滑出去一点再弹回(rubber-band)。多滑的距离 = 超出边界的
    /// 行数 ÷ 此值,值越大弹得越轻;翻页一次超出得多,弹得自然比逐行明显。`0` 视作 `1`。
    overshoot_damping: u32,

    /// 单次过冲上限,行的千分比(`1500` = 1.5 行);`0` = 关闭边界回弹。
    overshoot_max_permille: u32,
}
