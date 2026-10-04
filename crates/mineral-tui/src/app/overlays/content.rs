//! 应用实际挂载的浮层类型及其分发；通用栈不认识这些业务种类。

use super::input::OverlayInputs;
use crate::components::popup::{
    AudioSettingsOverlay, Chrome, ConfirmOverlay, DisconnectOverlay, DownloadOverlay, HelpOverlay,
    Overlay, OverlayResponse, PopMenu, QueueOverlay, TextPrompt,
};
use crate::render::theme::Theme;
use crate::runtime::action::Action;
use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect, widgets::Block};

/// 一种具体浮层。闭集 enum + 手动转发 trait 方法(强类型、无 `dyn`、契合内部结构化)。
pub(crate) enum AppOverlay {
    /// 通用单行输入。
    TextPrompt(TextPrompt),

    /// Runtime output device selection and active stream format.
    AudioSettings(AudioSettingsOverlay),

    /// 浮动播放队列。
    Queue(QueueOverlay),

    /// Flat Song downloads dock.
    Downloads(DownloadOverlay),

    /// 退出确认。
    Confirm(ConfirmOverlay),

    /// daemon 断连提示。
    Disconnect(DisconnectOverlay),

    /// 锚定弹出菜单(上下文操作 / 复制)。
    Menu(PopMenu),

    /// 键位 cheatsheet。
    Help(HelpOverlay),
}

impl AppOverlay {
    /// Creates an Audio settings popup waiting for the output device list.
    pub(crate) fn audio_settings() -> Self {
        Self::AudioSettings(AudioSettingsOverlay::new())
    }

    /// 浮动队列,光标定位到 `sel`(通常是在播歌下标)。
    pub(crate) fn queue(sel: usize) -> Self {
        Self::Queue(QueueOverlay::new(sel))
    }

    /// Flat Song downloads dock.
    pub(crate) fn downloads() -> Self {
        Self::Downloads(DownloadOverlay::new())
    }

    /// 退出确认。
    pub(crate) fn confirm() -> Self {
        Self::Confirm(ConfirmOverlay)
    }

    /// daemon 断连提示。
    pub(crate) fn disconnect() -> Self {
        Self::Disconnect(DisconnectOverlay)
    }

    /// 锚定弹出菜单。
    pub(crate) fn menu(menu: PopMenu) -> Self {
        Self::Menu(menu)
    }

    /// 键位 cheatsheet(目录与关闭提示在打开瞬间从 keymap 快照)。
    pub(crate) fn help(
        entries: Vec<crate::runtime::keymap::help::HelpEntry>,
        close_hint: Option<String>,
    ) -> Self {
        Self::Help(HelpOverlay::new(entries, close_hint))
    }
}

impl Overlay for AppOverlay {
    type Input<'a> = OverlayInputs<'a>;

    fn dependencies(
        &self,
        ctx: &OverlayInputs<'_>,
        env: crate::components::frame::FrameEnv<'_>,
        inputs: &mut crate::render::memo::Dependencies<'_>,
    ) {
        inputs.observe(&std::mem::discriminant(self));
        match self {
            Self::TextPrompt(o) => o.dependencies(&(), env, inputs),
            Self::AudioSettings(o) => o.dependencies(&ctx.output, env, inputs),
            Self::Queue(o) => o.dependencies(&ctx.queue, env, inputs),
            Self::Downloads(o) => o.dependencies(&ctx.downloads, env, inputs),
            Self::Confirm(o) => o.dependencies(&(), env, inputs),
            Self::Disconnect(o) => o.dependencies(&(), env, inputs),
            Self::Menu(o) => o.dependencies(&(), env, inputs),
            Self::Help(o) => o.dependencies(ctx.behavior, env, inputs),
        }
    }

    fn chrome(&self) -> Chrome {
        match self {
            Self::TextPrompt(o) => o.chrome(),
            Self::AudioSettings(o) => o.chrome(),
            Self::Queue(o) => o.chrome(),
            Self::Downloads(o) => o.chrome(),
            Self::Confirm(o) => o.chrome(),
            Self::Disconnect(o) => o.chrome(),
            Self::Menu(o) => o.chrome(),
            Self::Help(o) => o.chrome(),
        }
    }

    fn block(&self, ctx: &OverlayInputs<'_>, theme: &Theme, focused: bool) -> Block<'static> {
        match self {
            Self::TextPrompt(o) => o.block(&(), theme, focused),
            Self::AudioSettings(o) => o.block(&ctx.output, theme, focused),
            Self::Queue(o) => o.block(&ctx.queue, theme, focused),
            Self::Downloads(o) => o.block(&ctx.downloads, theme, focused),
            Self::Confirm(o) => o.block(&(), theme, focused),
            Self::Disconnect(o) => o.block(&(), theme, focused),
            Self::Menu(o) => o.block(&(), theme, focused),
            Self::Help(o) => o.block(ctx.behavior, theme, focused),
        }
    }

    fn prepare(
        &mut self,
        inner: Rect,
        ctx: &OverlayInputs<'_>,
        cx: &mut crate::components::frame::PrepareCx<'_>,
        reveal: crate::runtime::state::OverlayReveal,
    ) {
        match self {
            Self::TextPrompt(o) => o.prepare(inner, &(), cx, reveal),
            Self::AudioSettings(o) => o.prepare(inner, &ctx.output, cx, reveal),
            Self::Queue(o) => o.prepare(inner, &ctx.queue, cx, reveal),
            Self::Downloads(o) => o.prepare(inner, &ctx.downloads, cx, reveal),
            Self::Confirm(o) => o.prepare(inner, &(), cx, reveal),
            Self::Disconnect(o) => o.prepare(inner, &(), cx, reveal),
            Self::Menu(o) => o.prepare(inner, &(), cx, reveal),
            Self::Help(o) => o.prepare(inner, ctx.behavior, cx, reveal),
        }
    }

    fn render_content(
        &self,
        buf: &mut Buffer,
        inner: Rect,
        ctx: &OverlayInputs<'_>,
        theme: &Theme,
    ) {
        match self {
            Self::TextPrompt(o) => o.render_content(buf, inner, &(), theme),
            Self::AudioSettings(o) => o.render_content(buf, inner, &ctx.output, theme),
            Self::Queue(o) => o.render_content(buf, inner, &ctx.queue, theme),
            Self::Downloads(o) => o.render_content(buf, inner, &ctx.downloads, theme),
            Self::Confirm(o) => o.render_content(buf, inner, &(), theme),
            Self::Disconnect(o) => o.render_content(buf, inner, &(), theme),
            Self::Menu(o) => o.render_content(buf, inner, &(), theme),
            Self::Help(o) => o.render_content(buf, inner, ctx.behavior, theme),
        }
    }

    fn render_border(
        &self,
        buf: &mut Buffer,
        area: Rect,
        inner: Rect,
        ctx: &OverlayInputs<'_>,
        theme: &Theme,
    ) {
        match self {
            Self::TextPrompt(o) => o.render_border(buf, area, inner, &(), theme),
            Self::AudioSettings(o) => o.render_border(buf, area, inner, &ctx.output, theme),
            Self::Queue(o) => o.render_border(buf, area, inner, &ctx.queue, theme),
            Self::Downloads(o) => o.render_border(buf, area, inner, &ctx.downloads, theme),
            Self::Confirm(o) => o.render_border(buf, area, inner, &(), theme),
            Self::Disconnect(o) => o.render_border(buf, area, inner, &(), theme),
            Self::Menu(o) => o.render_border(buf, area, inner, &(), theme),
            Self::Help(o) => o.render_border(buf, area, inner, ctx.behavior, theme),
        }
    }

    fn on_key(&mut self, key: &KeyEvent, ctx: &OverlayInputs<'_>) -> OverlayResponse {
        match self {
            Self::TextPrompt(o) => o.on_key(key, &()),
            Self::AudioSettings(o) => o.on_key(key, &ctx.output),
            Self::Queue(o) => o.on_key(key, &ctx.queue),
            Self::Downloads(o) => o.on_key(key, &ctx.downloads),
            Self::Confirm(o) => o.on_key(key, &()),
            Self::Disconnect(o) => o.on_key(key, &()),
            Self::Menu(o) => o.on_key(key, &()),
            Self::Help(o) => o.on_key(key, ctx.behavior),
        }
    }

    fn on_action(&mut self, action: Action, ctx: &OverlayInputs<'_>) -> Option<OverlayResponse> {
        match self {
            Self::TextPrompt(o) => o.on_action(action, &()),
            Self::AudioSettings(o) => o.on_action(action, &ctx.output),
            Self::Queue(o) => o.on_action(action, &ctx.queue),
            Self::Downloads(o) => o.on_action(action, &ctx.downloads),
            Self::Confirm(o) => o.on_action(action, &()),
            Self::Disconnect(o) => o.on_action(action, &()),
            Self::Menu(o) => o.on_action(action, &()),
            Self::Help(o) => o.on_action(action, ctx.behavior),
        }
    }
}
