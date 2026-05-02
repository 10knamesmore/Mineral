//! 顶部状态行(1 行,无边框):左侧 tabs + 右侧 playback state。

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::state::{AppState, View};
use crate::theme::Theme;

/// 渲染状态行到给定 [`Rect`]。
pub fn draw(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let [left, right] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(40)]).areas(area);
    paint_left(frame, left, state, theme);
    paint_right(frame, right, state, theme);
}

fn paint_left(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let active_pl = state.view == View::Playlists;
    let active_lib = state.view == View::Library;
    let mut spans = vec![
        Span::styled(
            "▌ mineral v0.1.0  ",
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled("│  ", Style::new().fg(theme.surface1)),
        Span::styled("[playlists]", tab_style(active_pl, theme)),
        Span::raw("  "),
        Span::styled("[tracks]", tab_style(active_lib, theme)),
    ];
    if state.queue_open {
        spans.push(Span::raw("  "));
        spans.push(Span::styled("[3 queue]", tab_style(true, theme)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn tab_style(active: bool, theme: &Theme) -> Style {
    if active {
        Style::new().fg(theme.text).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.overlay)
    }
}

fn paint_right(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let pb = &state.playback;
    let (glyph, color, label) = if pb.playing {
        ("●", theme.green, "playing")
    } else {
        ("‖", theme.yellow, "paused")
    };
    let mut spans = Vec::<Span<'_>>::new();
    // 后台 task 计数:有任务在跑就显示「↓N」,跑完自动消失。
    if state.tasks_running > 0 {
        spans.push(Span::styled(
            format!("↓{} ", state.tasks_running),
            Style::new().fg(theme.peach),
        ));
    }
    spans.push(Span::styled(format!("{glyph} "), Style::new().fg(color)));
    spans.push(Span::styled(label, Style::new().fg(theme.subtext)));
    spans.push(Span::raw(" "));
    frame.render_widget(
        Paragraph::new(Line::from(spans)).alignment(Alignment::Right),
        area,
    );
}
