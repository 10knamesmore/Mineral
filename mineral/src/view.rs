//! 主帧渲染入口。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::Frame;

use crate::app::App;
use crate::components::overlay::confirm as confirm_overlay;
use crate::components::overlay::queue as queue_overlay;
use crate::components::{cmd_bar, lyrics, now_playing, sidebar, spectrum, top_status, transport};
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
        now_playing::draw(frame, right, &app.state, theme);
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

    if app.state.confirm_open {
        confirm_overlay::draw(frame, frame.area(), theme);
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
