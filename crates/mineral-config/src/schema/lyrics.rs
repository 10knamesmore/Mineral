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
    line_gap: usize,

    /// 当前行切换后整列平移 + 高亮交叉淡入的过渡时长(毫秒)。
    scroll_ms: u64,
}
