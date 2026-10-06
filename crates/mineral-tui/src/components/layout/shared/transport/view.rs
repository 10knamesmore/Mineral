//! 播放栏的只读输入；组件状态与播放模型分别借用。

use super::TransportBar;
use crate::components::frame::FrameEnv;
use crate::components::layout::shared::marquee::MarqueeCtx;
use crate::runtime::playback::Playback;
use ratatui::{Frame, layout::Rect, style::Color};

/// 播放栏需要的共享数据。
#[derive(Clone, Copy)]
pub(crate) struct TransportInput<'a> {
    /// 当前播放、缓冲与包络信息。
    pub(crate) playback: &'a Playback,
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

/// 播放栏的底色采样策略属于本次绘制输入，身份仍由原组件持有。
struct TransportPaint<'a> {
    /// 已准备播放栏及当前播放模型。
    view: &'a TransportView<'a>,

    /// 全屏标题从下层氛围背景淡出，普通页面使用主题底色。
    sample_background: bool,
}

impl crate::components::lifecycle::PaintView for TransportPaint<'_> {
    fn dependencies(
        &self,
        _area: Rect,
        _env: FrameEnv<'_>,
        inputs: &mut crate::render::memo::Dependencies<'_>,
    ) {
        self.view.dependencies(self.sample_background, inputs);
    }

    fn paint(&self, frame: &mut Frame<'_>, area: Rect, env: FrameEnv<'_>) {
        let fade = if self.sample_background {
            match crate::components::layout::shared::text::center_bg(frame, area) {
                color @ Color::Rgb(..) => color,
                _ => env.theme.base,
            }
        } else {
            env.theme.base
        };
        self.view.paint(frame, area, fade);
    }
}

impl TransportView<'_> {
    /// 为本次布局指定标题是否从下层画布采样底色。
    pub(crate) fn with_background(
        &self,
        sample_background: bool,
    ) -> impl crate::components::lifecycle::PaintView + '_ {
        TransportPaint {
            view: self,
            sample_background,
        }
    }

    /// 声明播放事实、局部反馈与标题时间依赖。
    fn dependencies(
        &self,
        sample_background: bool,
        inputs: &mut crate::render::memo::Dependencies<'_>,
    ) {
        let env = self.environment;
        let pb = self.input.playback;
        inputs.observe(&sample_background);
        inputs.observe(&pb.track);
        inputs.observe(&pb.position_ms);
        inputs.observe(&(pb.playing, pb.volume_pct, pb.mode));
        inputs.observe(&pb.media_info);
        inputs.observe(&pb.play_origin);
        inputs.observe(&pb.buffered_bps);
        inputs.observe(&(pb.sample_rate_hz, pb.engine_duration_ms, pb.prefetch));
        inputs.optional(pb.current_envelope());
        self.component.dependencies(inputs);
        self.component
            .title
            .dependencies(inputs, env.config.animation(), env.now);
    }

    /// 根据当前背景绘制标题、播放进度和已经更新的操作反馈。
    pub(crate) fn paint(&self, frame: &mut Frame<'_>, area: Rect, fade_to: Color) {
        let env = self.environment;
        let marquee = MarqueeCtx::new(
            &self.component.title,
            env.config.animation(),
            env.now,
            env.theme,
            fade_to,
        );
        super::paint::draw(
            frame,
            area,
            self.input.playback,
            self.component,
            &marquee,
            env,
        );
    }
}
