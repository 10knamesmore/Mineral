//! Library 视图渲染:展示当前选中歌单内的曲目。

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Row, StatefulWidget, Table, TableState};

use super::badge::search_badge;
use super::highlight::highlight_indices;
use crate::render::theme::Theme;
use crate::runtime::state::AppState;
use crate::runtime::view_model::SongView;

/// 曲目表格的列档,按面板宽度选(见 [`TrackLayout::for_width`])。
#[derive(Clone, Copy)]
enum TrackLayout {
    /// 宽档:♥ / # / title / artist / album / len,文本列比例 Fill(3:2:2)。
    Full,

    /// 窄档:♥ / # / title / len —— artist/album 放不下,退到「歌本身」。
    Song,
}

impl TrackLayout {
    /// 按面板宽度 `width` 选档。阈值 56:低于此 3 个文本列各分不到约 12 格,
    /// 退到只剩歌名(终端窄 → 面板窄 → 退列,与全局 compact 同源思路)。
    fn for_width(width: u16) -> Self {
        if width < 56 { Self::Song } else { Self::Full }
    }

    /// 表头单元格(与 [`Self::widths`] / [`build_row`] 的列集严格一致)。
    fn header_cells(self) -> Vec<Cell<'static>> {
        let mut cells = vec![Cell::from(""), Cell::from("#"), Cell::from("title")];
        if matches!(self, Self::Full) {
            cells.push(Cell::from("artist"));
            cells.push(Cell::from("album"));
        }
        cells.push(Cell::from("len"));
        cells
    }

    /// 列宽约束:定宽小列用 Length,文本列用比例 Fill。
    fn widths(self) -> Vec<Constraint> {
        match self {
            Self::Full => vec![
                Constraint::Length(1),
                Constraint::Length(4),
                Constraint::Fill(3),
                Constraint::Fill(2),
                Constraint::Fill(2),
                Constraint::Length(6),
            ],
            Self::Song => vec![
                Constraint::Length(1),
                Constraint::Length(4),
                Constraint::Fill(1),
                Constraint::Length(6),
            ],
        }
    }
}

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

    // 按面板宽度选列档:窄屏放不下 artist/album 时退到「歌本身」(♥ # title len)。
    let layout = TrackLayout::for_width(area.width);

    let header = Row::new(layout.header_cells())
        .style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));

    let rows: Vec<Row<'_>> = if let Some(row) = placeholder {
        vec![row]
    } else {
        tracks
            .iter()
            .enumerate()
            .map(|(i, sv)| build_row(i, sv, state, theme, layout))
            .collect()
    };

    let widths = layout.widths();

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
/// `layout` 决定列集:窄档省去 artist/album。
fn build_row<'a>(
    idx: usize,
    sv: &'a SongView,
    state: &AppState,
    theme: &Theme,
    layout: TrackLayout,
) -> Row<'a> {
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

    let name_hits = state.match_for(&sv.data.name).map(|m| m.hits);
    let title_cell = Cell::from(Line::from(highlight_indices(
        &sv.data.name,
        name_hits.as_deref().unwrap_or(&[]),
        Style::new().fg(theme.text),
        theme,
    )));

    let len = format_duration(sv.data.duration_ms);

    let mut cells = vec![love_cell, num_cell, title_cell];
    if matches!(layout, TrackLayout::Full) {
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
        let artist_hits = state.match_for(&artist).map(|m| m.hits);
        let album_hits = state.match_for(&album).map(|m| m.hits);
        cells.push(Cell::from(Line::from(highlight_indices(
            &artist,
            artist_hits.as_deref().unwrap_or(&[]),
            Style::new().fg(theme.subtext),
            theme,
        ))));
        cells.push(Cell::from(Line::from(highlight_indices(
            &album,
            album_hits.as_deref().unwrap_or(&[]),
            Style::new().fg(theme.overlay),
            theme,
        ))));
    }
    cells.push(Cell::from(len));
    Row::new(cells)
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
        // 占位文本落在 title 列(前两格留给 gutter / #),避免被 Length(1) 的 gutter 截成
        // 单字。两档列集的第 3 列都是 title,故位置通用。
        Row::new(vec![
            Cell::from(""),
            Cell::from(""),
            Cell::from(Span::styled("loading…", Style::new().fg(theme.overlay))),
        ])
    })
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::render::theme::Theme;
    use crate::runtime::state::View;

    /// 已选歌单 + 3 首曲目(CJK 歌名 / 收藏 / 当前在播标记)。
    #[test]
    fn library_with_tracks_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(80, 12))?;
        let state = crate::test_support::state_with_tracks()?;
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
        let mut t = Terminal::new(TestBackend::new(80, 12))?;
        let mut state = crate::test_support::state_with_playlists()?;
        state.view = View::Library;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!("曲目列表:选中歌单但曲目未到(loading)", t.backend());
        Ok(())
    }

    /// CJK 曲目(Chinese Football)在 Full 档多列里的宽字符对齐(width=80)—— 含最长的
    /// 「不是人人都能穿十号球衣」,验证 title/artist/album 三列宽字符不串列。
    #[test]
    fn library_cjk_tracks_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(80, 12))?;
        let state = crate::test_support::state_with_cjk_tracks()?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "曲目列表:CJK 曲目(Chinese Football)Full 档宽字符对齐",
            t.backend()
        );
        Ok(())
    }

    /// 窄面板(width=44 < 56)退到 Song 档:只剩 ♥ / # / title / len,artist/album 省去。
    #[test]
    fn library_narrow_song_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(44, 12))?;
        let state = crate::test_support::state_with_tracks()?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "曲目列表:窄面板退到 Song 档(只剩歌名,无 artist/album)",
            t.backend()
        );
        Ok(())
    }

    /// 带 album 数据的 Full 档(width=80):验证 album 列有内容时 title/artist/album
    /// 三列的渲染与对齐(其余 Full 档 fixture 的 album 为空,覆盖不到这条路径)。
    #[test]
    fn library_album_tracks_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(80, 12))?;
        let state = crate::test_support::state_with_album()?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "曲目列表:带 album 数据的 Full 档(title/artist/album 三列对齐)",
            t.backend()
        );
        Ok(())
    }
}
