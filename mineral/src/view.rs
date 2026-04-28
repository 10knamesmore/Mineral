//! 主帧渲染入口。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};
use ratatui::Frame;

use crate::app::App;
use crate::components::overlay::queue as queue_overlay;
use crate::components::{cmd_bar, lyrics, sidebar, spectrum, top_status, transport};
use crate::layout::{compute, Areas};
use crate::state::Focus;
use crate::theme::Theme;

/// 渲染一帧:计算布局,填充各面板。
pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let areas = compute(frame.area());
    paint(frame, &areas, app);
}

fn paint(frame: &mut Frame<'_>, areas: &Areas, app: &App) {
    let theme = &app.theme;
    top_status::draw(frame, areas.top_status, &app.state, theme);
    sidebar::draw(frame, areas.left, &app.state, theme);
    if let Some(right) = areas.right {
        paint_panel(frame, right, "now playing", theme);
    }
    transport::draw(frame, areas.transport, &app.state.playback, theme);
    if let Some(viz) = areas.viz {
        paint_viz(frame, viz, app, theme);
    }
    cmd_bar::draw(frame, areas.cmd_bar, &app.state, theme);

    if app.state.queue_open {
        let current_id = app.state.playback.track.as_ref().map(|t| &t.id);
        queue_overlay::draw(
            frame,
            frame.area(),
            &app.state.queue,
            app.state.queue_sel,
            current_id,
            theme,
            app.state.focus == Focus::Queue,
        );
    }
}

fn paint_viz(frame: &mut Frame<'_>, area: Rect, app: &App, theme: &Theme) {
    if area.height <= 4 {
        spectrum::draw(frame, area, &app.state.spectrum, theme);
        return;
    }
    let [spec_area, lyr_area] =
        Layout::vertical([Constraint::Percentage(58), Constraint::Percentage(42)]).areas(area);
    spectrum::draw(frame, spec_area, &app.state.spectrum, theme);
    lyrics::draw(frame, lyr_area, theme);
}

fn paint_panel(frame: &mut Frame<'_>, area: Rect, title: &str, theme: &Theme) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(format!(" {title} ")).style(Style::new().fg(theme.subtext)));
    frame.render_widget(block, area);
}
