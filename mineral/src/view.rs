//! 主帧渲染入口。

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::app::App;
use crate::components::sidebar;
use crate::layout::{compute, Areas};
use crate::theme::Theme;

/// 渲染一帧:计算布局,填充各面板。
pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let areas = compute(frame.area());
    paint(frame, &areas, app);
}

fn paint(frame: &mut Frame<'_>, areas: &Areas, app: &App) {
    let theme = &app.theme;
    paint_status_bar(frame, areas.top_status, theme);
    sidebar::draw(frame, areas.left, &app.state, theme);
    if let Some(right) = areas.right {
        paint_panel(frame, right, "now playing", theme);
    }
    paint_panel(frame, areas.transport, "transport", theme);
    if let Some(viz) = areas.viz {
        paint_panel(frame, viz, "spectrum / lyrics", theme);
    }
    paint_cmd_bar(frame, areas.cmd_bar, theme);
}

fn paint_panel(frame: &mut Frame<'_>, area: Rect, title: &str, theme: &Theme) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(format!(" {title} ")).style(Style::new().fg(theme.subtext)));
    frame.render_widget(block, area);
}

fn paint_status_bar(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let title = Paragraph::new(
        Line::from("▌ tuimu v0.1.0")
            .style(Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)),
    )
    .alignment(Alignment::Left);
    frame.render_widget(title, area);
}

fn paint_cmd_bar(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let hint = Paragraph::new(
        Line::from("j/k ↓↑ · h/l back/open · ↵ play · q quit")
            .style(Style::new().fg(theme.overlay)),
    )
    .alignment(Alignment::Left);
    frame.render_widget(hint, area);
}
