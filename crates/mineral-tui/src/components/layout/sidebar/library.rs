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

    // 视口行数 = 面板高 - 上下边框 - 表头;offset 跨帧持久(nvim 手感),滚动经缓动平移。
    let viewport = usize::from(area.height.saturating_sub(3));
    let offset = state.scroll_track.render_offset(
        state.sel_track,
        tracks.len(),
        viewport,
        state.scrolloff(),
        state.list_glide_ticks(),
    );
    let mut table_state = TableState::default()
        .with_offset(offset)
        .with_selected(Some(crate::runtime::scroll::pin_cursor(
            state.sel_track,
            offset,
            viewport,
        )));
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

/// 选中歌单尚未拿到 tracks 时返回 loading 行;tracks 已到但搜索零命中时返回
/// 「无匹配」行;正常情况返回 `None`(走 tracks 渲染)。
fn slot_placeholder<'a>(state: &AppState, theme: &Theme) -> Option<Row<'a>> {
    // 占位文本落在 title 列(前两格留给 gutter / #),避免被 Length(1) 的 gutter 截成
    // 单字。两档列集的第 3 列都是 title,故位置通用。
    let placeholder_row = |text: &'static str| {
        Row::new(vec![
            Cell::from(""),
            Cell::from(""),
            Cell::from(Span::styled(text, Style::new().fg(theme.overlay))),
        ])
    };
    if state.current_tracks_slot().is_none() {
        return state
            .selected_playlist()
            .map(|_| placeholder_row("loading…"));
    }
    if !state.search_q.is_empty() && state.filtered_tracks().is_empty() {
        return Some(placeholder_row("无匹配"));
    }
    None
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::render::theme::Theme;
    use crate::runtime::state::{AppState, View};

    /// 用本视图渲染入口画一帧(`60×12` ⇒ body 视口 = 12 - 边框 2 - 表头 1 = 9 行)。
    fn draw_lib(t: &mut Terminal<TestBackend>, state: &AppState) -> color_eyre::Result<()> {
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, state, &Theme::default());
        })?;
        Ok(())
    }

    /// 取 buffer 第 `y` 行拼成字符串(首个 body 行在 y=2:边框 0 + 表头 1 之后)。
    fn row(t: &Terminal<TestBackend>, y: u16) -> String {
        let buf = t.backend().buffer();
        (0..buf.area.width)
            .filter_map(|x| buf.cell((x, y)).map(ratatui::buffer::Cell::symbol))
            .collect()
    }

    /// nvim 手感回归:G 滚到底后向上走,光标在 scrolloff 安全区内时视口纹丝不动
    /// (修掉「光标钉死视口底边、列表粘着光标滚」的旧行为);越过安全边界才上滚。
    #[test]
    fn library_bottom_then_up_keeps_viewport() -> color_eyre::Result<()> {
        let mut app = crate::test_support::app_with_long_library(30, /*sel_track*/ 29)?;
        let mut t = Terminal::new(TestBackend::new(60, 12))?;
        // 收敛到底:offset = len - 视口 = 21(glide 默认 ≈18 拍,放足 40 帧)。
        for _ in 0..40 {
            draw_lib(&mut t, &app.state)?;
        }
        let bottom_first = row(&t, 2);
        // 安全区 [offset+3, offset+9-1-3] = [24, 26]:逐行上移视口不滚。
        for sel in (24..=28).rev() {
            app.state.sel_track = sel;
            draw_lib(&mut t, &app.state)?;
            assert_eq!(
                row(&t, 2),
                bottom_first,
                "sel={sel} 在安全区内,视口不应滚动"
            );
        }
        // 越过上安全边界:视口开始上滚,首行变化。
        app.state.sel_track = 23;
        for _ in 0..40 {
            draw_lib(&mut t, &app.state)?;
        }
        assert_ne!(row(&t, 2), bottom_first, "越过安全边界视口应上滚");
        Ok(())
    }

    /// `G`/`gg` 首末行大跳的视口平移也走缓动:跳转后的首帧停在中间位置
    /// (既非起点也非终点),多帧后才收敛——证明大跳不是瞬跳。
    #[test]
    fn library_jump_first_last_animates() -> color_eyre::Result<()> {
        let mut app = crate::test_support::app_with_long_library(30, /*sel_track*/ 0)?;
        let mut t = Terminal::new(TestBackend::new(60, 12))?;
        draw_lib(&mut t, &app.state)?;
        let top_first = row(&t, 2);

        // G 跳末行:首帧视口应离开顶部但未到底(缓动中段)。
        app.state.sel_track = 29;
        draw_lib(&mut t, &app.state)?;
        let mid = row(&t, 2);
        assert_ne!(mid, top_first, "G 跳转首帧视口应已起步");
        for _ in 0..40 {
            draw_lib(&mut t, &app.state)?;
        }
        let bottom_first = row(&t, 2);
        assert_ne!(mid, bottom_first, "G 跳转首帧不应一步到底(应是缓动中段)");

        // gg 跳回首行:同样多帧缓动收敛回顶。
        app.state.sel_track = 0;
        draw_lib(&mut t, &app.state)?;
        let mid_back = row(&t, 2);
        assert_ne!(mid_back, bottom_first, "gg 跳转首帧视口应已起步");
        assert_ne!(mid_back, top_first, "gg 跳转首帧不应一步到顶");
        for _ in 0..40 {
            draw_lib(&mut t, &app.state)?;
        }
        assert_eq!(row(&t, 2), top_first, "gg 多帧后收敛回顶");
        Ok(())
    }

    /// scrolloff 边距:从顶下移越过安全区后,选中行稳定停在距视口底 `scrolloff`(3)行处。
    #[test]
    fn library_scrolloff_margin_at_bottom_edge() -> color_eyre::Result<()> {
        let mut app = crate::test_support::app_with_long_library(30, /*sel_track*/ 0)?;
        let mut t = Terminal::new(TestBackend::new(60, 12))?;
        draw_lib(&mut t, &app.state)?;
        // 下移到 10:offset 收敛到 10+3+1-9 = 5,选中行落在 y = 2 + (10-5) = 7,
        // 距 body 末行(y=10)恰 3 行。
        app.state.sel_track = 10;
        for _ in 0..40 {
            draw_lib(&mut t, &app.state)?;
        }
        let sel_row = (2..=10_u16).find(|&y| row(&t, y).contains('▌'));
        assert_eq!(sel_row, Some(7), "选中行应停在距视口底 scrolloff 行处");
        Ok(())
    }

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

    /// 曲目已到位但搜索零命中 → 表内「无匹配」占位行(而非纯空白)。
    #[test]
    fn library_search_no_match_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(80, 12))?;
        let mut state = crate::test_support::state_with_tracks()?;
        state.search_q = "zzz".to_owned();
        draw_lib(&mut t, &state)?;
        crate::test_support::assert_snap!("曲目列表:搜索零命中(表内「无匹配」占位行)", t.backend());
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
