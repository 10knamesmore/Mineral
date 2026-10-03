//! 只有居中上边框标题与单行文本的通用输入弹窗。

use super::component::{Chrome, Overlay, OverlayAction, OverlayResponse, base_block};
use crate::components::text_input::TextInput;
use crate::render::theme::Theme;
use crate::runtime::state::AppState;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::Style,
    widgets::Block,
};

/// 通用单行弹窗；展示配置只有标题。
pub(crate) struct TextPrompt {
    /// 上边框标题。
    title: String,

    /// 文本与光标状态。
    input: TextInput,

    /// 每次确认时校验文本并生成响应；取消不调用，随弹窗销毁。
    on_submit: Box<dyn Fn(&str) -> OverlayResponse>,
}

impl TextPrompt {
    /// 创建空输入框并绑定提交行为；回调返回 [`OverlayResponse::CloseAndDo`] 关闭并执行。
    /// 校验不通过时可用 [`OverlayResponse::Do`] 发出提示，保留输入继续编辑。
    pub(crate) fn new(
        title: impl Into<String>,
        on_submit: impl Fn(&str) -> OverlayResponse + 'static,
    ) -> Self {
        Self {
            title: title.into(),
            input: TextInput::new(),
            on_submit: Box::new(on_submit),
        }
    }
    /// 由调用方预填当前名称。
    pub(crate) fn set_text(&mut self, text: String) {
        self.input.set_text(text);
    }
    /// 粘贴文本，不解释为确认或取消。
    pub(crate) fn paste(&mut self, text: &str) {
        self.input.paste(text);
    }
}

impl Overlay for TextPrompt {
    fn chrome(&self) -> Chrome {
        Chrome {
            pct_w: 50,
            pct_h: 0,
            min_w: 24,
            min_h: 3,
            max_w: 60,
            max_h: 3,
            animated: true,
            dock: false,
            anchor: None,
            align: None,
        }
    }
    fn block(&self, _ctx: &AppState, theme: &Theme, _focused: bool) -> Block<'static> {
        base_block(theme)
            .title(self.title.clone())
            .title_alignment(Alignment::Center)
            .border_style(Style::new().fg(theme.accent))
    }
    fn render_content(&self, buf: &mut Buffer, inner: Rect, _ctx: &AppState, theme: &Theme) {
        self.input.render(buf, inner, Style::new().fg(theme.text));
    }
    fn on_key(&mut self, key: &KeyEvent, _ctx: &AppState) -> OverlayResponse {
        match key.code {
            KeyCode::Enter => (self.on_submit)(self.input.text()),
            KeyCode::Esc => OverlayResponse::Do(OverlayAction::CloseTop),
            _ => {
                self.input.on_key(key);
                OverlayResponse::Consumed
            }
        }
    }
}
