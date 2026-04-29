//! Playlists 视图渲染。

use mineral_model::SourceKind;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::state::AppState;
use crate::theme::Theme;
use crate::view_model::PlaylistView;

/// 渲染 Playlists 视图到给定 [`Rect`]。
pub fn draw(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(vec![
            Span::styled(" playlists ", Style::new().fg(theme.subtext)),
            search_badge(&state.search_q, theme),
        ]));

    let header = Row::new(vec![
        Cell::from("name"),
        Cell::from("source"),
        Cell::from("length"),
        Cell::from("items"),
    ])
    .style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));

    let rows: Vec<Row<'_>> = state
        .filtered_playlists()
        .into_iter()
        .map(|p| build_row(p, state, theme))
        .collect();

    let widths = [
        Constraint::Min(12),
        Constraint::Length(11),
        Constraint::Length(8),
        Constraint::Length(10),
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
    table_state.select(Some(state.sel_playlist));
    frame.render_stateful_widget(table, area, &mut table_state);
}

fn build_row<'a>(p: &'a PlaylistView, state: &AppState, theme: &Theme) -> Row<'a> {
    let total_ms = state.total_duration_ms_of(&p.data.id);
    let len_label = if total_ms == 0 {
        String::from("—")
    } else {
        let total_min = total_ms / 60_000;
        format!("{}h {:02}m", total_min / 60, total_min % 60)
    };
    let count_label = format!("{} items", p.data.track_count);

    Row::new(vec![
        Cell::from(Span::styled(
            p.data.name.clone(),
            Style::new().fg(theme.text),
        )),
        Cell::from(Span::styled(
            channel_label(p.data.source),
            channel_style(p.data.source, theme),
        )),
        Cell::from(Span::styled(len_label, Style::new().fg(theme.subtext))),
        Cell::from(Span::styled(count_label, Style::new().fg(theme.overlay))),
    ])
}

fn channel_label(src: SourceKind) -> &'static str {
    match src {
        SourceKind::Netease => "♫ netease",
        SourceKind::Local => "□ local",
        #[cfg(feature = "mock")]
        SourceKind::Mock => "▒ mock",
    }
}

fn channel_style(src: SourceKind, theme: &Theme) -> Style {
    match src {
        SourceKind::Netease => Style::new().fg(theme.red),
        SourceKind::Local => Style::new().fg(theme.subtext),
        #[cfg(feature = "mock")]
        SourceKind::Mock => Style::new().fg(theme.overlay),
    }
}

fn search_badge<'a>(q: &'a str, theme: &Theme) -> Span<'a> {
    if q.is_empty() {
        Span::raw("")
    } else {
        Span::styled(format!("/{q}"), Style::new().fg(theme.peach))
    }
}
