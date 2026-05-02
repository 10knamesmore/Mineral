//! Library 视图右栏:程序化封面(以专辑名为种子) + KV + 底部 ▶ 当前曲目。

use mineral_model::SongId;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui_image::picker::Picker;

use crate::components::cover_image;
use crate::playback::format_ms;
use crate::state::AppState;
use crate::theme::Theme;
use crate::view_model::SongView;

/// 渲染曲目详情(right pane)到 `area`。
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    sv: &SongView,
    current_id: Option<&SongId>,
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

    let [cover_area, kv_area, current_strip] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(inner);

    let seed = sv
        .data
        .album
        .as_ref()
        .map_or_else(|| sv.data.name.clone(), |a| a.name.clone());
    cover_image::render_or_fallback(
        frame,
        cover_area,
        sv.data.cover_url.as_ref(),
        state,
        picker,
        theme,
        &seed,
    );

    let len = format_ms(sv.data.duration_ms);
    let love_label = if sv.loved { "♥ loved" } else { "♡ —" };
    let love_color = if sv.loved { theme.red } else { theme.overlay };

    let kv = vec![
        Line::from(vec![
            Span::raw(" "),
            Span::styled("length: ", Style::new().fg(theme.overlay)),
            Span::styled(format!("{len:<10}"), Style::new().fg(theme.text)),
            Span::styled("plays:  ", Style::new().fg(theme.overlay)),
            Span::styled(format!("{}", sv.plays), Style::new().fg(theme.text)),
        ]),
        Line::from(vec![
            Span::raw(" "),
            Span::styled("love: ", Style::new().fg(theme.overlay)),
            Span::styled(love_label, Style::new().fg(love_color)),
        ]),
    ];
    frame.render_widget(Paragraph::new(kv), kv_area);

    let is_current = current_id.is_some_and(|cid| cid == &sv.data.id);
    let style = if is_current {
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.overlay)
    };
    let artist = sv
        .data
        .artists
        .first()
        .map(|a| a.name.clone())
        .unwrap_or_default();
    let strip = Line::from(format!(" ▶ {} — {artist} ", sv.data.name)).style(style);
    frame.render_widget(Paragraph::new(strip), current_strip);
}
