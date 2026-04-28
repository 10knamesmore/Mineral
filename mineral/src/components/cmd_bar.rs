//! 命令 / 帮助行(1 行,无边框)。
//!
//! - inactive:左侧紧凑键位提示 + 右侧 hint(自动消)
//! - active:bg surface0 + 左侧 prefix `/` 或 `:` + buffer + 光标块 +
//!   右侧 `↵ run · esc cancel`

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::cmd::CmdMode;
use crate::state::AppState;
use crate::theme::Theme;

/// 默认键位提示(inactive 时左侧)。
const KEYS_HINT: &str =
    "j/k ↓↑ · h/l back/open · ↵ play · ␣ pause · m mode · s sort · / search · : cmd · Tab queue · q quit";

/// 渲染 cmd / help 行到给定 [`Rect`]。
pub fn draw(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    if state.cmd_mode.is_some() {
        paint_active(frame, area, state, theme);
    } else {
        paint_inactive(frame, area, state, theme);
    }
}

fn paint_active(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let Some(mode) = state.cmd_mode else {
        return;
    };
    let prefix = if mode == CmdMode::Search { "/" } else { ":" };
    frame.render_widget(Block::new().style(Style::new().bg(theme.surface0)), area);

    let [left, right] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(22)]).areas(area);

    let line = Line::from(vec![
        Span::styled(
            format!(" {prefix}"),
            Style::new().fg(theme.peach).add_modifier(Modifier::BOLD),
        ),
        Span::styled(state.cmd_buffer.clone(), Style::new().fg(theme.text)),
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

fn paint_inactive(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let hint = state.hint.as_ref().map(|(s, _)| s.as_str());
    let keys = Line::from(KEYS_HINT).style(Style::new().fg(theme.overlay));
    if let Some(h) = hint {
        let [l_area, r_area] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(40)]).areas(area);
        frame.render_widget(Paragraph::new(keys), l_area);
        let hint_line = Line::from(format!(" {h} "))
            .style(Style::new().fg(theme.peach).add_modifier(Modifier::ITALIC));
        frame.render_widget(
            Paragraph::new(hint_line).alignment(Alignment::Right),
            r_area,
        );
    } else {
        frame.render_widget(Paragraph::new(keys), area);
    }
}
