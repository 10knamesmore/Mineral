//! 播放栏的只读输入；组件状态与播放模型分别借用。

use super::TransportBar;
use crate::components::frame::FrameEnv;
use crate::components::layout::shared::{marquee::MarqueeCtx, waveform::WaveformCtx};
use crate::render::palette::CoverPalette;
use crate::runtime::playback::Playback;
use ratatui::{Frame, layout::Rect, style::Color};

/// 播放栏需要的共享数据。
#[derive(Clone, Copy)]
pub(crate) struct TransportInput<'a> {
    /// 当前播放、缓冲与包络信息。
    pub(crate) playback: &'a Playback,

    /// 当前歌曲已经应用的封面色板。
    pub(crate) palette: Option<&'a CoverPalette>,
}

/// 播放栏在这一帧的只读状态。
pub(crate) struct TransportView<'a> {
    /// 自己持有的标题和操作反馈。
    component: &'a TransportBar,

    /// 播放模型借用。
    input: TransportInput<'a>,

    /// 当前配置、主题与时间。
    environment: FrameEnv<'a>,
}

impl TransportBar {
    /// 组装显示输入，调用者无需了解播放栏内部有哪些动画。
    pub(crate) fn view<'a>(
        &'a self,
        input: TransportInput<'a>,
        environment: FrameEnv<'a>,
    ) -> TransportView<'a> {
        TransportView {
            component: self,
            input,
            environment,
        }
    }
}

impl TransportView<'_> {
    /// 根据当前背景绘制标题、播放进度和已经更新的操作反馈。
    pub(crate) fn paint(&self, frame: &mut Frame<'_>, area: Rect, fade_to: Color) {
        let env = self.environment;
        let marquee = MarqueeCtx::new(
            &self.component.title,
            env.config.tui().animation(),
            env.now,
            env.theme,
            fade_to,
        );
        let waveform = WaveformCtx::new(
            env.config.tui().waveform(),
            self.input.playback,
            self.input.palette,
            env.theme,
        );
        super::paint::draw(
            frame,
            area,
            self.input.playback,
            self.component,
            &marquee,
            &waveform,
            env.theme,
        );
    }
}
