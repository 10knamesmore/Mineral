//! 底部状态行(1 行,无边框)。
//!
//! - 非搜索态:左侧 keys hint + 右侧临时 hint(自动消)
//! - 搜索态:bg surface0 + 左侧 `/` + 当前 search_q + 光标块 + 右侧 `↵ run · esc cancel`

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::state::AppState;
use crate::theme::Theme;

/// 默认键位提示(非搜索态时左侧)。
const KEYS_HINT: &str =
    "j/k ↓↑ · h/l back/open · ↵ play · ␣ pause · m mode · / search · Tab queue · q quit";

/// 渲染状态行到给定 [`Rect`]。
pub fn draw(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    if state.search_mode {
        paint_active(frame, area, state, theme);
    } else {
        paint_inactive(frame, area, state, theme);
    }
}

fn paint_active(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    frame.render_widget(Block::new().style(Style::new().bg(theme.surface0)), area);

    let [left, right] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(22)]).areas(area);

    let line = Line::from(vec![
        Span::styled(
            " /",
            Style::new().fg(theme.peach).add_modifier(Modifier::BOLD),
        ),
        Span::styled(state.search_q.clone(), Style::new().fg(theme.text)),
        Span::styled(
            "█",
            Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), left);

    let right_line = Line::from(" ↵ run · esc cancel ").style(Style::new().fg(theme.overlay));
    frame.render_widget(
        Paragraph::new(right_line).alignment(Alignment::Right),
        right,
    );
}

fn paint_inactive(frame: &mut Frame<'_>, area: Rect, _state: &AppState, theme: &Theme) {
    let keys = Line::from(KEYS_HINT).style(Style::new().fg(theme.overlay));
    frame.render_widget(Paragraph::new(keys), area);
}
