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
    /// 为本次布局选择歌词模式，不改变组件实例或准备状态。
    pub(crate) fn with_mode(
        &self,
        mode: super::LyricMode,
    ) -> impl crate::components::lifecycle::PaintView + '_ {
        LyricsPaint { view: self, mode }
    }

    /// 歌词面板比较当前歌词、位置和已采样外观，不以原始时钟作为统一失效条件。
    fn dependencies(
        &self,
        motion: super::LyricMode,
        inputs: &mut crate::render::memo::Dependencies<'_>,
    ) {
        inputs.observe(&motion);
        inputs.optional(self.input.song);
        inputs.optional(self.input.lyrics);
        inputs.observe(&(self.input.position_ms, self.input.trust));
        inputs.observe(&self.panel.extra);
        inputs.observe(&self.panel.extra_press.strength());
        inputs.observe(&(self.manual_anchor(), self.manual_focus()));
        self.panel
            .colors
            .dependencies(inputs, self.frame.config.lyrics(), motion, self.frame.now);
    }

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

/// 同一歌词实例在紧凑或沉浸布局中的绘制选项。
struct LyricsPaint<'a> {
    /// 已准备状态及共享歌词借用。
    view: &'a LyricsView<'a>,

    /// 本次布局的歌词显示模式。
    mode: super::LyricMode,
}

impl crate::components::lifecycle::PaintView for LyricsPaint<'_> {
    fn dependencies(
        &self,
        _area: ratatui::layout::Rect,
        _env: FrameEnv<'_>,
        inputs: &mut crate::render::memo::Dependencies<'_>,
    ) {
        self.view.dependencies(self.mode, inputs);
    }

    fn paint(
        &self,
        frame: &mut ratatui::Frame<'_>,
        area: ratatui::layout::Rect,
        env: FrameEnv<'_>,
    ) {
        super::draw(frame, area, self.view, env.theme, self.mode);
    }
}

impl<'a> LyricsInput<'a> {
    /// 当前曲目的歌词行，保持模型的原始借用。
    pub(super) fn lines(self) -> Option<&'a [mineral_model::LyricLine]> {
        self.lyrics.map(|lyrics| lyrics.lines.as_slice())
    }
}
