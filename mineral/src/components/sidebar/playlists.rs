//! Playlists 视图渲染。

use mineral_model::SourceKind;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState};
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

    let items: Vec<ListItem<'_>> = state
        .filtered_playlists()
        .into_iter()
        .map(|p| ListItem::new(playlist_row(p, theme)))
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::new()
                .bg(theme.surface0)
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▌ ");

    let mut list_state = ListState::default();
    list_state.select(Some(state.sel_playlist));
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn playlist_row<'a>(p: &'a PlaylistView, theme: &Theme) -> Line<'a> {
    let total_min = p.total_duration_ms() / 60_000;
    let len_label = format!("{}h {:02}m", total_min / 60, total_min % 60);
    let count_label = format!("{} items", p.data.track_count);

    Line::from(vec![
        Span::styled(p.data.name.clone(), Style::new().fg(theme.text)),
        Span::raw("  "),
        Span::styled(
            channel_label(p.data.source),
            channel_style(p.data.source, theme),
        ),
        Span::raw("  "),
        Span::styled(len_label, Style::new().fg(theme.subtext)),
        Span::raw("  "),
        Span::styled(count_label, Style::new().fg(theme.overlay)),
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
