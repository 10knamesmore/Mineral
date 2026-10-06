//! 浮层与应用事件的接线，业务判断不进入容器生命周期。

use super::AppOverlay;
use crate::components::popup::{
    AudioSettingsOverlay, Overlay, OverlayResponse, OverlayStack, TextPrompt, render_overlay,
};
use crate::render::theme::Theme;
use crate::runtime::action::Action;
use crate::runtime::state::AppState;
use crossterm::event::KeyEvent;
use mineral_model::{MediaUrl, SourceKind};
use ratatui::{Frame, layout::Rect};

impl OverlayStack<AppOverlay> {
    /// 栈顶输入框独占键盘和粘贴。
    pub(crate) fn text_prompt_mut(&mut self) -> Option<&mut TextPrompt> {
        match self.active_mut()? {
            AppOverlay::TextPrompt(prompt) => Some(prompt),
            _ => None,
        }
    }

    /// 当前内容是否独占文本输入。
    pub(crate) fn in_text_input(&self) -> bool {
        matches!(self.active(), Some(AppOverlay::TextPrompt(_)))
    }

    /// 把设备查询完成事件交给仍然打开的设备浮层。
    pub(crate) fn audio_settings_mut(&mut self) -> Option<&mut AudioSettingsOverlay> {
        self.live_mut().find_map(|content| match content {
            AppOverlay::AudioSettings(popup) => Some(popup),
            _ => None,
        })
    }

    /// 配置变更后调整队列展开速度，保留当前相位。
    pub(crate) fn retempo_queue_search(&mut self, ticks: u16) {
        for content in self.iter_mut() {
            if let AppOverlay::Queue(queue) = content {
                queue.retempo_search_expansion(ticks);
            }
        }
    }

    /// 更新应用内容自己的动画；容器负责开合与卸载。
    pub(crate) fn tick(&mut self) {
        self.advance(|content| {
            if let AppOverlay::Queue(queue) = content {
                queue.tick_search_expansion();
            }
        });
    }

    /// 为当前输入接收者接入应用数据与键位映射结果。
    pub(crate) fn dispatch_key(
        &mut self,
        key: &KeyEvent,
        action: Option<Action>,
        ctx: &AppState,
    ) -> Option<OverlayResponse> {
        let content = self.active_mut()?;
        let ctx = &super::input::all(ctx);
        if let Some(action) = action
            && let Some(response) = content.on_action(action, ctx)
        {
            return Some(response);
        }
        Some(content.on_key(key, ctx))
    }

    /// 准备各具体浮层的数据和显示需求。
    pub(crate) fn prepare_content(
        &mut self,
        area: Rect,
        ctx: &mut AppState,
        theme: &Theme,
        advance: bool,
    ) {
        let mut cx = crate::components::frame::PrepareCx {
            frame: super::input::environment(ctx, theme).frame,
            images: crate::image::ImageNeeds::new(ctx.resources.images.ready()),
            motion: crate::runtime::scroll::list::ScrollMotion::Frozen,
            image_phase: if !ctx.ui.browse.fullscreen.settled()
                || !ctx.ui.channel_search.active.settled()
            {
                crate::image::ImageRenderPhase::Resizing
            } else {
                crate::image::ImageRenderPhase::Stable
            },
            advance,
        };
        let inputs = super::input::all(ctx);
        self.prepare(
            area,
            ctx.cfg.layout(),
            ctx.ui.browse.fullscreen.on(),
            |content, layout| {
                let inner = content
                    .block(&inputs, theme, layout.focused)
                    .inner(layout.area);
                content.prepare(inner, &inputs, &mut cx, layout.reveal);
            },
        );
        ctx.resources.images.reconcile(cx.images.finish());
    }

    /// 组合每层的只读视图并提交到目标 buffer。
    pub(crate) fn render(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        ctx: &super::input::OverlayInputs<'_>,
        env: crate::components::popup::OverlayEnv<'_>,
    ) {
        self.visit(|content, scale, focused| {
            render_overlay(frame, area, content, scale, focused, ctx, env)
        });
    }

    /// 当前队列选中项的真实出现位置。
    pub(crate) fn active_queue_cursor(&self, ctx: &AppState) -> Option<usize> {
        match self.active()? {
            AppOverlay::Queue(queue) => queue.raw_cursor(&super::input::queue(ctx)),
            _ => None,
        }
    }

    /// 尚未离场的队列组件声明的预取候选。
    pub(crate) fn queue_cover_candidates(&self, ctx: &AppState) -> Vec<(SourceKind, MediaUrl)> {
        self.live()
            .filter_map(|content| match content {
                AppOverlay::Queue(queue) => Some(queue),
                _ => None,
            })
            .flat_map(|queue| queue.cover_candidates(&super::input::queue(ctx)))
            .collect()
    }

    /// 断连提示使应用进入等待退出状态。
    pub(crate) fn is_disconnected(&self) -> bool {
        self.iter()
            .any(|content| matches!(content, AppOverlay::Disconnect(_)))
    }

    /// 新的队列镜像到达后调整各队列组件。
    pub(crate) fn clamp_queue(&mut self, len: usize) {
        for content in self.iter_mut() {
            if let AppOverlay::Queue(queue) = content {
                queue.clamp(len);
            }
        }
    }

    /// 下载明细只在有消费者时订阅。
    pub(crate) fn has_downloads(&self) -> bool {
        self.iter()
            .any(|content| matches!(content, AppOverlay::Downloads(_)))
    }

    /// 下载快照到达后调整对应组件的光标。
    pub(crate) fn clamp_downloads(&mut self, len: usize) {
        for content in self.iter_mut() {
            if let AppOverlay::Downloads(downloads) = content {
                downloads.clamp(len);
            }
        }
    }

    /// 查询帮助组件是否仍挂载，包含离场过程。
    #[cfg(test)]
    pub(crate) fn has_help(&self) -> bool {
        self.iter()
            .any(|content| matches!(content, AppOverlay::Help(_)))
    }

    /// 测试通过应用入口观察队列光标。
    #[cfg(test)]
    pub(crate) fn queue_sel(&self) -> Option<usize> {
        self.iter().find_map(|content| match content {
            AppOverlay::Queue(queue) => Some(queue.cursor()),
            _ => None,
        })
    }
}
