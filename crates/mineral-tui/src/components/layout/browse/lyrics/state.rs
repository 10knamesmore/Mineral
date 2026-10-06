//! 歌词面板的显示态:副歌词档(原文 / 翻译 / 罗马音)+ 全屏手动滚动的脱离态。

use crate::components::layout::browse::lyrics::LyricColors;
use crate::render::control_press::ControlPress;
use mineral_model::SongId;

use super::glide::LyricGlide;
use crate::runtime::state::LyricExtra;

/// 歌词面板持有自己的切档、颜色和手动滚动状态。
pub struct LyricsPanel {
    /// 副歌词(翻译 / 罗马音)显示档,由 `t` 键循环。
    pub extra: LyricExtra,

    /// 切换副歌词时，面板按键提示的底色反馈。
    pub(crate) extra_press: ControlPress,

    /// 两种面板中原文字词的颜色动画，跨播放状态变化保留当前显示色。
    pub(crate) colors: LyricColors,

    /// 全屏歌词手动滚动的「脱离播放」态;`None` = 附着态(渲染跟随播放,逐行时间驱动平滑)。
    pub(crate) scroll: Option<LyricGlide>,

    /// 手动滚动绑定的歌;换歌即清滚动偏移。
    pub(crate) scroll_song: Option<SongId>,
}

impl LyricsPanel {
    /// 构造初始显示态(原文档、附着态、未绑定歌)。
    pub(crate) fn new() -> Self {
        Self {
            extra: LyricExtra::None,
            extra_press: ControlPress::default(),
            colors: LyricColors::default(),
            scroll: None,
            scroll_song: None,
        }
    }
}

impl LyricsPanel {
    /// 循环副歌词档:`None → Translation → Romanization → None`,跳过当前歌为空的档。
    /// 翻译 / 罗马音都缺时停在 `None`。
    pub(crate) fn cycle_extra(
        &mut self,
        input: super::LyricsInput<'_>,
        animation: &crate::config::AnimationConfig,
    ) {
        let has_trans = input
            .lyrics
            .is_some_and(mineral_model::Lyrics::has_translation);
        let has_roma = input
            .lyrics
            .is_some_and(mineral_model::Lyrics::has_romanization);
        self.extra = match self.extra {
            LyricExtra::None if has_trans => LyricExtra::Translation,
            LyricExtra::None if has_roma => LyricExtra::Romanization,
            LyricExtra::None => LyricExtra::None,
            LyricExtra::Translation if has_roma => LyricExtra::Romanization,
            LyricExtra::Translation => LyricExtra::None,
            LyricExtra::Romanization => LyricExtra::None,
        };
        if has_trans || has_roma {
            self.extra_press.trigger(animation);
            mineral_log::debug!(target: "tui::lyrics", extra = ?self.extra,
                "lyric extra switched");
        }
    }
}
