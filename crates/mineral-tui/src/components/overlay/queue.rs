//! 浮动 queue 面板。

use mineral_model::{Song, SongId};
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Clear, Row, Table, TableState};
use ratatui::Frame;

use crate::components::overlay::centered_rect;
use crate::playback::format_ms;
use crate::theme::Theme;

/// 渲染 queue 浮层,以 `area`(主帧区域)为参考居中。
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    queue: &[Song],
    sel: usize,
    current_id: Option<&SongId>,
    theme: &Theme,
    focused: bool,
) {
    let panel = centered_rect(area, 60, 70, 40, 12, 96, 32);
    paint_shadow(frame, panel, area, theme);
    frame.render_widget(Clear, panel);

    let border_color = if focused {
        theme.accent
    } else {
        theme.surface1
    };
    let pos = position_label(sel, queue.len());
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border_color))
        .style(Style::new().bg(theme.mantle))
        .title(Line::from(vec![
            Span::styled(" queue · floating ", Style::new().fg(theme.subtext)),
            Span::styled(" press q to close ", Style::new().fg(theme.overlay)),
        ]))
        .title_bottom(Line::from(pos).style(Style::new().fg(theme.overlay)))
        .title_bottom(
            Line::from(" ↵ play  ·  Tab/q/esc close ")
                .right_aligned()
                .style(Style::new().fg(theme.overlay)),
        );

    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from("track"),
        Cell::from("len"),
    ])
    .style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));

    let rows: Vec<Row<'_>> = queue
        .iter()
        .enumerate()
        .map(|(i, s)| build_row(i, s, current_id, theme))
        .collect();

    let widths = [
        Constraint::Length(3),
        Constraint::Min(12),
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

    let mut state = TableState::default();
    state.select(Some(sel));
    frame.render_stateful_widget(table, panel, &mut state);
}

fn build_row<'a>(idx: usize, s: &'a Song, current_id: Option<&SongId>, theme: &Theme) -> Row<'a> {
    let is_current = current_id.is_some_and(|cid| cid == &s.id);
    let num = if is_current {
        Cell::from(Span::styled("▶", Style::new().fg(theme.accent)))
    } else {
        Cell::from(format!("{idx}"))
    };
    let artist = s
        .artists
        .first()
        .map(|a| a.name.clone())
        .unwrap_or_default();
    let track_str = format!("{} — {artist}", s.name);
    let len = format_ms(s.duration_ms);
    Row::new(vec![num, Cell::from(track_str), Cell::from(len)])
}

fn position_label(sel: usize, total: usize) -> String {
    if total == 0 {
        " 0 / 0 ".to_owned()
    } else {
        format!(" {} / {total} ", sel.saturating_add(1).min(total))
    }
}

fn paint_shadow(frame: &mut Frame<'_>, panel: Rect, area: Rect, theme: &Theme) {
    let off_x = panel.x.saturating_add(2);
    let off_y = panel.y.saturating_add(1);
    let max_x = area.x.saturating_add(area.width);
    let max_y = area.y.saturating_add(area.height);
    if off_x >= max_x || off_y >= max_y {
        return;
    }
    let w = panel.width.min(max_x.saturating_sub(off_x));
    let h = panel.height.min(max_y.saturating_sub(off_y));
    let shadow = Rect::new(off_x, off_y, w, h);
    let bg = Block::new().style(Style::new().bg(theme.crust));
    frame.render_widget(Clear, shadow);
    frame.render_widget(bg, shadow);
}
