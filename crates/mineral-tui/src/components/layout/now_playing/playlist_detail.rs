//! Playlists 视图右栏:程序化封面 + KV(tracks/length/source/...) + footer。

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui_image::picker::Picker;

use crate::components::layout::cover_image;
use crate::render::theme::Theme;
use crate::runtime::state::AppState;
use crate::runtime::view_model::PlaylistView;

/// 渲染歌单详情(right pane)到 `area`。
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    p: &PlaylistView,
    state: &AppState,
    picker: &Picker,
    theme: &Theme,
) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(" selected ").style(Style::new().fg(theme.subtext)));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 4 || inner.width < 8 {
        return;
    }

    let [cover_area, kv_area, footer] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(2),
        Constraint::Length(1),
    ])
    .areas(inner);

    cover_image::render_or_fallback(
        frame,
        cover_area,
        p.data.cover_url.as_ref(),
        state,
        picker,
        theme,
        &p.data.name,
    );

    let total_ms = state.total_duration_ms_of(&p.data.id);
    let len_label = if total_ms == 0 {
        String::from("—")
    } else {
        let total_min = total_ms / 60_000;
        format!("{}h {:02}m", total_min / 60, total_min % 60)
    };

    let src = p.data.source();
    let kv = vec![
        Line::from(vec![
            Span::raw(" "),
            Span::styled("tracks: ", Style::new().fg(theme.overlay)),
            Span::styled(
                format!("{:<10}", p.data.track_count),
                Style::new().fg(theme.text),
            ),
            Span::styled("length: ", Style::new().fg(theme.overlay)),
            Span::styled(len_label, Style::new().fg(theme.text)),
        ]),
        Line::from(vec![
            Span::raw(" "),
            Span::styled("source: ", Style::new().fg(theme.overlay)),
            Span::styled(
                src.label(),
                Style::new().fg(theme.source_color(src.palette())),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(kv), kv_area);

    let help = Line::from(" ↵ open · h back ").style(Style::new().fg(theme.overlay));
    frame.render_widget(Paragraph::new(help), footer);
}
