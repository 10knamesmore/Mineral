//! 歌词段(挂在 `TuiConfig` 下):全屏沉浸态行距 + 滚动手感。

use serde::Deserialize;

/// 歌词配置。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[derive(Clone, Debug, Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct LyricsConfig {
    /// 全屏沉浸态行与行之间垫的空行数(`0` = 紧凑)。
    fullscreen_line_gap: usize,

    /// 非全屏紧凑态行与行之间垫的空行数(`0` = 紧凑)。
    compact_line_gap: usize,

    /// 当前行切换后整列平移 + 高亮交叉淡入的过渡时长(毫秒)。
    scroll_ms: u64,

    /// 单行档手动滚动一次移动的行数(`<C-d>` / `<C-u>`)。不依赖终端实际高度。
    line_scroll_rows: usize,

    /// 多行档手动滚动一次移动的行数(`<C-f>` / `<C-b>`)。
    page_scroll_rows: usize,

    /// 有时间戳歌手动滚走后,空闲多久(毫秒)自动平滑回到跟随当前行。无时间戳歌不回锚。
    reattach_ms: u32,
}
