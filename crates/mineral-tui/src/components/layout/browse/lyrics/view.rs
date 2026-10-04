//! 歌词面板接收的播放事实与只读视图。

use super::LyricsPanel;
use crate::components::frame::FrameEnv;
use crate::runtime::playback::SyncTrust;
use crate::runtime::state::LyricExtra;
use mineral_model::{Lyrics, SongId};

/// 当前曲目的歌词与播放位置，不包含播放器或歌曲库的修改能力。
#[derive(Clone, Copy)]
pub(crate) struct LyricsInput<'a> {
    /// 当前歌曲身份，切歌时用于重新绑定颜色动画。
    pub(crate) song: Option<&'a SongId>,

    /// 已加载的完整歌词集合。
    pub(crate) lyrics: Option<&'a Lyrics>,

    /// 当前播放位置，单位毫秒。
    pub(crate) position_ms: u64,

    /// 歌词时间轴是否可以跟随播放。
    pub(crate) trust: SyncTrust,
}

/// 歌词组件已更新状态和本次共享输入的只读借用。
pub(crate) struct LyricsView<'a> {
    /// 组件自己的显示状态。
    pub(super) panel: &'a LyricsPanel,

    /// 本次歌词与播放事实。
    pub(super) input: LyricsInput<'a>,

    /// 本次配置、主题与时间。
    pub(super) frame: FrameEnv<'a>,
}

impl LyricsPanel {
    /// 构造可重复绘制的只读视图。
    pub(crate) fn view<'a>(
        &'a self,
        input: LyricsInput<'a>,
        frame: FrameEnv<'a>,
    ) -> LyricsView<'a> {
        LyricsView {
            panel: self,
            input,
            frame,
        }
    }
}

impl LyricsView<'_> {
    /// 当前曲目确实具备的副歌词档。
    pub(super) fn active_extra(&self) -> Option<LyricExtra> {
        let lyrics = self.input.lyrics?;
        match self.panel.extra {
            LyricExtra::Translation if lyrics.has_translation() => Some(LyricExtra::Translation),
            LyricExtra::Romanization if lyrics.has_romanization() => Some(LyricExtra::Romanization),
            _ => None,
        }
    }

    /// 副歌词切换入口是否有可用内容。
    pub(super) fn has_extra(&self) -> bool {
        self.input
            .lyrics
            .is_some_and(|lyrics| lyrics.has_translation() || lyrics.has_romanization())
    }

    /// 本组件当前的手动滚动坐标。
    pub(super) fn manual_anchor(&self) -> Option<i64> {
        self.panel
            .scroll
            .as_ref()
            .map(super::glide::LyricGlide::pos_milli)
    }

    /// 手动滚动的目标原文行。
    pub(super) fn manual_focus(&self) -> Option<usize> {
        self.panel
            .scroll
            .as_ref()
            .and_then(|scroll| usize::try_from(scroll.target_line()).ok())
    }
}

impl<'a> LyricsInput<'a> {
    /// 当前曲目的歌词行，保持模型的原始借用。
    pub(super) fn lines(self) -> Option<&'a [mineral_model::LyricLine]> {
        self.lyrics.map(|lyrics| lyrics.lines.as_slice())
    }
}
