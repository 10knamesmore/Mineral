//! Library 视图渲染:展示当前选中歌单内的曲目。

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::mock::SongView;
use crate::state::AppState;
use crate::theme::Theme;

/// 渲染 Library 视图到给定 [`Rect`]。
pub fn draw(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let title = state.selected_playlist().map_or_else(
        || "library".to_owned(),
        |p| format!("library / {}", p.data.name),
    );

    let tracks = state.filtered_tracks();
    let total_min = tracks.iter().map(|s| s.data.duration_ms).sum::<u64>() / 60_000;

    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(vec![
            Span::styled(format!(" {title} "), Style::new().fg(theme.subtext)),
            search_badge(&state.search_q, theme),
        ]))
        .title_bottom(
            Line::from(format!("{total_min}m total"))
                .right_aligned()
                .style(Style::new().fg(theme.overlay)),
        );

    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from("title"),
        Cell::from("artist"),
        Cell::from("album"),
        Cell::from("plays"),
        Cell::from("len"),
    ])
    .style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));

    let rows: Vec<Row<'_>> = tracks
        .iter()
        .enumerate()
        .map(|(i, sv)| build_row(i, sv, state, theme))
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Min(12),
        Constraint::Length(16),
        Constraint::Length(14),
        Constraint::Length(6),
        Constraint::Length(6),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(
            Style::new()
                .bg(theme.surface0)
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▌ ");

    let mut table_state = TableState::default();
    table_state.select(Some(state.sel_track));
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn build_row<'a>(idx: usize, sv: &'a SongView, state: &AppState, theme: &Theme) -> Row<'a> {
    let is_current = state.current.as_ref().is_some_and(|c| c.id == sv.data.id);

    let num_cell = if is_current {
        Cell::from(Span::styled("♫", Style::new().fg(theme.accent)))
    } else {
        Cell::from(format!("{}", idx + 1))
    };

    let title_cell = if sv.loved {
        Cell::from(Line::from(vec![
            Span::styled("♥ ", Style::new().fg(theme.red)),
            Span::raw(sv.data.name.clone()),
        ]))
    } else {
        Cell::from(sv.data.name.clone())
    };

    let artist = sv
        .data
        .artists
        .first()
        .map(|a| a.name.clone())
        .unwrap_or_default();
    let album = sv
        .data
        .album
        .as_ref()
        .map(|a| a.name.clone())
        .unwrap_or_default();
    let plays = format!("{}", sv.plays);
    let len = format_duration(sv.data.duration_ms);

    Row::new(vec![
        num_cell,
        title_cell,
        Cell::from(Span::styled(artist, Style::new().fg(theme.subtext))),
        Cell::from(Span::styled(album, Style::new().fg(theme.overlay))),
        Cell::from(plays),
        Cell::from(len),
    ])
}

fn format_duration(ms: u64) -> String {
    let secs = ms / 1000;
    let m = secs / 60;
    let s = secs % 60;
    format!("{m}:{s:02}")
}

fn search_badge<'a>(q: &'a str, theme: &Theme) -> Span<'a> {
    if q.is_empty() {
        Span::raw("")
    } else {
        Span::styled(format!("/{q}"), Style::new().fg(theme.peach))
    }
}
