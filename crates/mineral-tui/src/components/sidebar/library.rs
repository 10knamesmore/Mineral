//! Library 视图渲染:展示当前选中歌单内的曲目。

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Row, StatefulWidget, Table, TableState};

use super::badge::search_badge;
use super::highlight::highlight;
use crate::state::AppState;
use crate::theme::Theme;
use crate::view_model::SongView;

/// 渲染 Library 视图到给定 [`Buffer`](正常渲染与离屏过渡合成共用此入口)。
pub fn render_to(buf: &mut Buffer, area: Rect, state: &AppState, theme: &Theme) {
    let title = state.selected_playlist().map_or_else(
        || "tracks".to_owned(),
        |p| format!("tracks / {}", p.data.name),
    );

    let tracks = state.filtered_tracks();
    let total_min = tracks.iter().map(|s| s.data.duration_ms).sum::<u64>() / 60_000;
    let placeholder = slot_placeholder(state, theme);
    let pos = position_label(state.sel_track, tracks.len());

    let mut title_spans = vec![Span::styled(
        format!(" {title} "),
        Style::new().fg(theme.subtext),
    )];
    title_spans.extend(search_badge(state, theme));

    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(title_spans))
        .title_bottom(Line::from(pos).style(Style::new().fg(theme.overlay)))
        .title_bottom(
            Line::from(format!("{total_min}m total"))
                .right_aligned()
                .style(Style::new().fg(theme.overlay)),
        );

    let header = Row::new(vec![
        Cell::from(""),
        Cell::from("#"),
        Cell::from("title"),
        Cell::from("artist"),
        Cell::from("album"),
        Cell::from("len"),
    ])
    .style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));

    let rows: Vec<Row<'_>> = if let Some(row) = placeholder {
        vec![row]
    } else {
        tracks
            .iter()
            .enumerate()
            .map(|(i, sv)| build_row(i, sv, state, theme))
            .collect()
    };

    let widths = [
        Constraint::Length(1),
        Constraint::Length(4),
        Constraint::Min(12),
        Constraint::Length(16),
        Constraint::Length(14),
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
    StatefulWidget::render(table, area, buf, &mut table_state);
}

/// 把一首歌组装成 library 表格的一行(loved 标记 / ♫ 当前歌 / 高亮搜索词)。
fn build_row<'a>(idx: usize, sv: &'a SongView, state: &AppState, theme: &Theme) -> Row<'a> {
    let is_current = state.current.as_ref().is_some_and(|c| c.id == sv.data.id);

    // 像 vim signcolumn 一样的 gutter:loved 显 ♥,否则空。永远占一格,
    // 不抖动后续列。
    let love_cell = if sv.loved {
        Cell::from(Span::styled("♥", Style::new().fg(theme.red)))
    } else {
        Cell::from("")
    };

    let num_cell = if is_current {
        Cell::from(Span::styled("♫", Style::new().fg(theme.accent)))
    } else {
        Cell::from(format!("{idx}"))
    };

    let q = state.search_q.as_str();
    let title_cell = Cell::from(Line::from(highlight(
        &sv.data.name,
        q,
        Style::new().fg(theme.text),
        theme,
    )));

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
    let len = format_duration(sv.data.duration_ms);

    Row::new(vec![
        love_cell,
        num_cell,
        title_cell,
        Cell::from(Line::from(highlight(
            &artist,
            q,
            Style::new().fg(theme.subtext),
            theme,
        ))),
        Cell::from(Line::from(highlight(
            &album,
            q,
            Style::new().fg(theme.overlay),
            theme,
        ))),
        Cell::from(len),
    ])
}

/// 把时长 ms 格式化成 `m:ss`(library 行右侧使用)。
fn format_duration(ms: u64) -> String {
    let secs = ms / 1000;
    let m = secs / 60;
    let s = secs % 60;
    format!("{m}:{s:02}")
}

/// 拼 ` n / total ` 的 footer 标签;空列表显示 `0 / 0`。
fn position_label(sel: usize, total: usize) -> String {
    if total == 0 {
        " 0 / 0 ".to_owned()
    } else {
        format!(" {} / {total} ", sel.saturating_add(1).min(total))
    }
}

/// 选中歌单尚未拿到 tracks 时返回 loading 行,已到位返回 `None`(走正常 tracks 渲染)。
fn slot_placeholder<'a>(state: &AppState, theme: &Theme) -> Option<Row<'a>> {
    if state.current_tracks_slot().is_some() {
        return None;
    }
    state.selected_playlist().map(|_| {
        Row::new(vec![Cell::from(Span::styled(
            "loading…",
            Style::new().fg(theme.overlay),
        ))])
    })
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::state::View;
    use crate::theme::Theme;

    /// 已选歌单 + 3 首曲目(CJK 歌名 / 收藏 / 当前在播标记)。
    #[test]
    fn library_with_tracks_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        let state = crate::test_support::state_with_tracks();
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "曲目列表:EndSerenading 前 3 曲(♫ 当前 / ♥ 收藏)",
            t.backend()
        );
        Ok(())
    }

    /// 选中歌单但曲目未到(tracks_cache 空)→ loading 态。
    #[test]
    fn library_loading_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        let mut state = crate::test_support::state_with_playlists();
        state.view = View::Library;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!("曲目列表:选中歌单但曲目未到(loading)", t.backend());
        Ok(())
    }

    /// CJK 曲目(Chinese Football)在多列表格里的宽字符对齐 / 截断 —— 含最长的
    /// 「不是人人都能穿十号球衣」。
    #[test]
    fn library_cjk_tracks_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        let state = crate::test_support::state_with_cjk_tracks();
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "曲目列表:CJK 曲目(Chinese Football)宽字符对齐",
            t.backend()
        );
        Ok(())
    }
}
