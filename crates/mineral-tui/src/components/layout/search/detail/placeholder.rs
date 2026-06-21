//! detail 列表区的占位渲染:数据未到货的旋转 loading,与到货为空的静态空态——两者刻意
//! 用不同观感(旋转 vs 静态)区分「在飞」与「就是没有」。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::components::layout::shared::spinner;
use crate::render::theme::Theme;
use crate::runtime::state::AppState;

/// 数据未到货:旋转 spinner + `loading` 文案(逐帧旋转,与「到货为空」的静态空态区分开——
/// 旋转即「在飞」)。`glyph` 由调用方按配置帧 + 帧计数取(见 [`loading_glyph`])。
pub(super) fn draw_loading(buf: &mut Buffer, area: Rect, glyph: &str, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let style = Style::new().fg(theme.overlay);
    let text = if glyph.is_empty() {
        "  loading".to_owned()
    } else {
        format!("  {glyph} loading")
    };
    Widget::render(
        Paragraph::new(Line::from(Span::styled(text, style))),
        Rect::new(area.x, area.y, area.width, 1),
        buf,
    );
}

/// 已到货但当前区为空:静态空态文案(明确「就是没有」,不画旋转 spinner——与 loading 区分)。
pub(super) fn draw_empty(buf: &mut Buffer, area: Rect, label: &str, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let style = Style::new().fg(theme.overlay).add_modifier(Modifier::DIM);
    Widget::render(
        Paragraph::new(Line::from(Span::styled(format!("  {label}"), style))),
        Rect::new(area.x, area.y, area.width, 1),
        buf,
    );
}

/// 当前 loading spinner 字形(配置 `animation.spinner_frames` + `SearchPage` 帧计数)。
pub(super) fn loading_glyph(state: &AppState) -> &str {
    spinner::glyph(
        state.cfg.tui().animation().spinner_frames(),
        state.channel_search.spinner_counter(),
    )
}
