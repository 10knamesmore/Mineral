//! 歌词面板的显示态:副歌词档(原文 / 翻译 / 罗马音)+ 全屏手动滚动的脱离态。
//!
//! 只持状态;操作它的方法(切档、滚动、回锚)因要跨读 playback/fullscreen/配置,
//! 留在组合根([`AppState`](crate::runtime::state::AppState))。

use mineral_model::SongId;

use super::super::LyricExtra;
use super::glide::LyricGlide;

/// 歌词显示态([`AppState`](crate::runtime::state::AppState) 的歌词面板域)。
pub struct LyricView {
    /// 副歌词(翻译 / 罗马音)显示档,由 `t` 键循环。
    pub extra: LyricExtra,

    /// 全屏歌词手动滚动的「脱离播放」态;`None` = 附着态(渲染跟随播放,逐行时间驱动平滑)。
    pub(crate) scroll: Option<LyricGlide>,

    /// 手动滚动绑定的歌;换歌即清滚动偏移。
    pub(crate) scroll_song: Option<SongId>,
}

impl LyricView {
    /// 构造初始显示态(原文档、附着态、未绑定歌)。
    pub(crate) fn new() -> Self {
        Self {
            extra: LyricExtra::None,
            scroll: None,
            scroll_song: None,
        }
    }
}
