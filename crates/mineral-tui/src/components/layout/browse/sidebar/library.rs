//! Library 视图渲染:展示当前选中歌单内的曲目。

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Row, Table};

use mineral_model::SourceKind;

use super::badge::search_badge;
use super::highlight::{alias_suffix, highlight_indices};
use crate::components::layout::shared::marquee::{
    MarqueeCtx, RowMarquee, resolve_column_widths, row_marquee,
};
use crate::components::layout::shared::scroll_table::render_scroll_table;
use crate::render::theme::Theme;
use crate::runtime::format::format_ms_opt;
use crate::runtime::marquee::Slot;
use crate::runtime::scroll::list::ScrollMotion;
use crate::runtime::state::AppState;
use crate::runtime::view_model::SongView;

/// 曲目表格的列布局:宽度档(是否放得下 artist/album)× 是否聚合面(mineral 源)。
#[derive(Clone, Copy)]
struct TrackLayout {
    /// 宽档:♥ / # / title / artist / album / len,文本列比例 Fill(3:2:2);
    /// `false` 为窄档 ♥ / # / title / len——artist/album 放不下,退到「歌本身」。
    full: bool,

    /// 聚合面(source = mineral 的跨源歌单,如全源收藏合集):宽档在 album 后插
    /// per-song `source` 徽标列;窄档插不起 11 格列,改为序号染该行歌曲的源色(与
    /// queue 同一手法)。普通单源歌单为 `false`,天然无需 per-song source 表示。
    aggregate: bool,
}

impl TrackLayout {
    /// 按面板宽度与曲目集合选布局。宽度阈值:普通面 56(低于此 3 个文本列各分不到约 12 格,
    /// 退到只剩歌名);聚合面还要塞 11 格 source 列,阈值抬到 68(56 + 列宽及间隔),否则
    /// 56~67 格会挤瘦 artist/album——窄档改由序号染色承担源表示,不硬塞列。
    fn new(width: u16, aggregate: bool) -> Self {
        let full_threshold = if aggregate { 68 } else { 56 };
        Self {
            full: width >= full_threshold,
            aggregate,
        }
    }

    /// 表头单元格(与 [`Self::widths`] / [`build_row`] 的列集严格一致)。
    fn header_cells(self) -> Vec<Cell<'static>> {
        let mut cells = vec![Cell::from(""), Cell::from("#"), Cell::from("title")];
        if self.full {
            cells.push(Cell::from("artist"));
            cells.push(Cell::from("album"));
            if self.aggregate {
                cells.push(Cell::from("source"));
            }
        }
        cells.push(Cell::from("len"));
        cells
    }

    /// 列宽约束:定宽小列用 Length,文本列用比例 Fill;source 列与
    /// playlists sidebar 的同名列等宽。
    fn widths(self) -> Vec<Constraint> {
        let mut widths = vec![Constraint::Length(1), Constraint::Length(4)];
        if self.full {
            widths.extend([
                Constraint::Fill(3),
                Constraint::Fill(2),
                Constraint::Fill(2),
            ]);
            if self.aggregate {
                widths.push(Constraint::Length(11));
            }
        } else {
            widths.push(Constraint::Fill(1));
        }
        widths.push(Constraint::Length(6));
        widths
    }
}

/// 渲染 Library 视图到给定 [`Buffer`](正常渲染与离屏过渡合成共用此入口)。
pub fn render_to(buf: &mut Buffer, area: Rect, state: &AppState, theme: &Theme) {
    let title = state.selected_playlist().map_or_else(
        || "tracks".to_owned(),
        |p| format!("tracks / {}", p.data.name),
    );

    let tracks = state.filtered_tracks();
    // 未知时长的曲目不计入合计(只反映已知部分)。
    let total_min = tracks
        .iter()
        .filter_map(|s| s.data.duration_ms)
        .sum::<u64>()
        / 60_000;
    let placeholder = slot_placeholder(state, theme);
    let pos = position_label(state.browse.nav.track.sel(), tracks.len());

    // 左上角 source 徽标:标出当前歌单挂靠的来源(聚合面挂靠 mineral,单源面挂靠其真实
    // 来源),与 sidebar playlists 面的 source 列同色,离开 sidebar(全屏)时仍能辨源。
    let mut title_spans = Vec::new();
    if let Some(p) = state.selected_playlist() {
        let src = p.data.source();
        title_spans.push(Span::styled(
            format!(" {}", src.label()),
            Style::new().fg(crate::render::theme::resolve_source_color(
                theme,
                state.cfg.sources(),
                src,
            )),
        ));
    }
    title_spans.push(Span::styled(
        format!(" {title} "),
        Style::new().fg(theme.subtext),
    ));
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

    // 按面板宽度 × 是否聚合面选布局:窄屏放不下 artist/album 时退到「歌本身」
    // (♥ # title len);聚合面(source = mineral 的跨源歌单)额外带 per-song source
    // 表示。跨源的只有 mineral 源歌单,故看歌单 source 而非遍历曲目。
    let aggregate = state
        .selected_playlist()
        .is_some_and(|p| p.data.source() == SourceKind::MINERAL);
    let layout = TrackLayout::new(area.width, aggregate);

    let header = Row::new(layout.header_cells())
        .style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));

    let widths = layout.widths();
    // 表格选中行的 fade 实际会被 row_highlight_style 整行 fg 盖掉(刻意保留整行
    // accent,见 MarqueeCtx::fade_to 注);fade_to 仍按其底色给,不误导插值方向。
    let marquee_ctx = MarqueeCtx::new(state, theme, /*fade_to*/ theme.surface0);
    // Table 带边框 block:列在 inner(左右各 -1)上求解;选中符 "▌ " 恒占 2 列。
    let title_w = resolve_column_widths(area.width.saturating_sub(2), &widths, 2)
        .get(2)
        .copied()
        .unwrap_or(0);
    let sel = state.browse.nav.track.sel();
    let rows: Vec<Row<'_>> = if let Some(row) = placeholder {
        vec![row]
    } else {
        tracks
            .iter()
            .enumerate()
            .map(|(i, sv)| {
                let marquee = row_marquee(i == sel, &marquee_ctx, Slot::BrowseSelected, title_w);
                build_row(i, sv, state, theme, layout, marquee)
            })
            .collect()
    };

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
    // 全屏 morph 中面板 rect 是插值瞬态:只读展示(Frozen),收缩中的 viewport 不得改写
    // 滚动目标(否则回浏览态时选中行换屏上位置 + 多一段平移)。
    let viewport = usize::from(area.height.saturating_sub(3));
    let motion = if state.browse.fullscreen.at_min() {
        ScrollMotion::Advancing {
            scrolloff: state.scrolloff(),
            glide_ticks: state.list_glide_ticks(),
        }
    } else {
        ScrollMotion::Frozen
    };
    render_scroll_table(
        buf,
        area,
        table,
        &state.browse.nav.track,
        tracks.len(),
        viewport,
        motion,
    );
}

/// 把一首歌组装成 library 表格的一行(loved 标记 / ♫ 当前歌 / 高亮搜索词)。
/// `layout` 决定列集:窄档省去 artist/album。
fn build_row<'a>(
    idx: usize,
    sv: &'a SongView,
    state: &AppState,
    theme: &Theme,
    layout: TrackLayout,
    marquee: Option<RowMarquee<'_>>,
) -> Row<'a> {
    let is_current = state
        .player
        .current
        .as_ref()
        .is_some_and(|c| c.id == sv.data.id);

    // 像 vim signcolumn 一样的 gutter:loved 显 ♥,否则空。永远占一格,
    // 不抖动后续列。
    let love_cell = if sv.loved {
        Cell::from(Span::styled("♥", Style::new().fg(theme.red)))
    } else {
        Cell::from("")
    };

    let num_cell = if is_current {
        Cell::from(Span::styled("♫", Style::new().fg(theme.accent)))
    } else if layout.aggregate && !layout.full {
        // 窄档聚合面:source 列插不起,序号承担源表示(染该行歌曲的源色)。
        let src_color = crate::render::theme::resolve_source_color(
            theme,
            state.cfg.sources(),
            sv.data.source(),
        );
        Cell::from(Span::styled(format!("{idx}"), Style::new().fg(src_color)))
    } else {
        Cell::from(format!("{idx}"))
    };

    let name_hits = state.browse.search.match_for(&sv.data.name).map(|m| m.hits);
    let mut title_spans = highlight_indices(
        &sv.data.name,
        name_hits.as_deref().unwrap_or(&[]),
        Style::new().fg(theme.text),
        theme,
    );
    // alias(译名 / 副标题)是歌名的暗色括注后缀;命中字符与主字段同款 search_hit
    // 高亮。hits 是相对 alias 文本的 char 下标。
    if let Some(alias) = sv.data.alias.as_deref() {
        let alias_hits = state.browse.search.match_for(alias).map(|m| m.hits);
        title_spans.extend(alias_suffix(
            alias,
            alias_hits.as_deref().unwrap_or(&[]),
            theme,
        ));
    }
    let title_cell = match marquee {
        Some(m) => Cell::from(
            m.ctx
                .line(title_spans, m.slot, &sv.data.id.qualified(), m.title_w),
        ),
        None => Cell::from(Line::from(title_spans)),
    };

    let len = format_ms_opt(sv.data.duration_ms);

    let mut cells = vec![love_cell, num_cell, title_cell];
    if layout.full {
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
        let artist_hits = state.browse.search.match_for(&artist).map(|m| m.hits);
        let album_hits = state.browse.search.match_for(&album).map(|m| m.hits);
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
        if layout.aggregate {
            let src = sv.data.source();
            cells.push(Cell::from(Span::styled(
                src.label(),
                Style::new().fg(crate::render::theme::resolve_source_color(
                    theme,
                    state.cfg.sources(),
                    src,
                )),
            )));
        }
    }
    cells.push(Cell::from(len));
    let row = Row::new(cells);
    if sv.data.unavailable {
        row.style(theme.unavailable_row())
    } else {
        row
    }
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
    if !state.browse.search.query().is_empty() && state.filtered_tracks().is_empty() {
        return Some(placeholder_row("无匹配"));
    }
    None
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::TrackLayout;
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
            app.state.browse.nav.track.set_sel(sel);
            draw_lib(&mut t, &app.state)?;
            assert_eq!(
                row(&t, 2),
                bottom_first,
                "sel={sel} 在安全区内,视口不应滚动"
            );
        }
        // 越过上安全边界:视口开始上滚,首行变化。
        app.state.browse.nav.track.set_sel(23);
        for _ in 0..40 {
            draw_lib(&mut t, &app.state)?;
        }
        assert_ne!(row(&t, 2), bottom_first, "越过安全边界视口应上滚");
        Ok(())
    }

    /// 全屏 morph 回归:形变中面板以插值瞬态 rect 渲染,期间滚动目标不得被
    /// 收缩中的 viewport 改写——回浏览态后视口首行与进全屏前一致(选中行屏上
    /// 位置不变、无平移)。
    #[test]
    fn fullscreen_morph_keeps_scroll_target() -> color_eyre::Result<()> {
        use crate::render::anim::Toggle;

        let mut app = crate::test_support::app_with_long_library(60, /*sel_track*/ 40)?;
        let mut t = Terminal::new(TestBackend::new(60, 24))?;
        for _ in 0..40 {
            draw_lib(&mut t, &app.state)?;
        }
        let before = app.state.browse.nav.track.scroll_target();
        assert!(before > 0, "前置:视口已滚到深处");

        // 进入 morph(fullscreen 离开 at_min),面板高度逐帧收缩地渲染。
        let mut fs = Toggle::new(8);
        fs.set(true);
        fs.tick();
        app.state.browse.fullscreen = fs;
        for h in (2..20_u16).rev() {
            let mut small = Terminal::new(TestBackend::new(60, h))?;
            draw_lib(&mut small, &app.state)?;
        }
        assert_eq!(
            app.state.browse.nav.track.scroll_target(),
            before,
            "morph 期间滚动目标不得被瞬态 viewport 改写"
        );

        // 回浏览态:渲染收敛后仍在原 offset(无重定目标 = 无平移)。
        app.state.browse.fullscreen = Toggle::new(8);
        for _ in 0..10 {
            draw_lib(&mut t, &app.state)?;
        }
        assert_eq!(
            app.state.browse.nav.track.scroll_target(),
            before,
            "回浏览态视口首行应与进全屏前一致"
        );
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
        app.state.browse.nav.track.set_sel(29);
        draw_lib(&mut t, &app.state)?;
        let mid = row(&t, 2);
        assert_ne!(mid, top_first, "G 跳转首帧视口应已起步");
        for _ in 0..40 {
            draw_lib(&mut t, &app.state)?;
        }
        let bottom_first = row(&t, 2);
        assert_ne!(mid, bottom_first, "G 跳转首帧不应一步到底(应是缓动中段)");

        // gg 跳回首行:同样多帧缓动收敛回顶。
        app.state.browse.nav.track.set_sel(0);
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
        app.state.browse.nav.track.set_sel(10);
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

    /// 左上角 source 徽标:单源歌单染该歌单真实 source 色,聚合面(mineral)染 mineral 色,
    /// 与 sidebar playlists 面 source 列同一套 [`resolve_source_color`]。
    #[test]
    fn library_title_badge_matches_source_color() -> color_eyre::Result<()> {
        use mineral_model::SourceKind;

        use crate::render::theme::resolve_source_color;

        let theme = Theme::default();
        let fg_of = |t: &Terminal<TestBackend>, ch: &str| -> Option<ratatui::style::Color> {
            let buf = t.backend().buffer();
            (0..buf.area.width)
                .find_map(|x| buf.cell((x, 0)).filter(|c| c.symbol() == ch).map(|c| c.fg))
        };

        let single = crate::test_support::state_with_tracks()?;
        let mut t = Terminal::new(TestBackend::new(80, 12))?;
        draw_lib(&mut t, &single)?;
        assert_eq!(
            fg_of(&t, "♫"),
            Some(resolve_source_color(
                &theme,
                single.cfg.sources(),
                SourceKind::NETEASE
            )),
            "单源歌单徽标应染该歌单的 netease 色"
        );

        let mixed = crate::test_support::state_with_mixed_tracks()?;
        let mut t = Terminal::new(TestBackend::new(80, 12))?;
        draw_lib(&mut t, &mixed)?;
        assert_eq!(
            fg_of(&t, "◆"),
            Some(resolve_source_color(
                &theme,
                mixed.cfg.sources(),
                SourceKind::MINERAL
            )),
            "聚合面徽标应染歌单自身的 mineral 色(不受 per-song 真实 source 影响)"
        );
        Ok(())
    }

    /// 歌名带别名(译名):title 后追加暗色 ` (alias)` 后缀,其余行不受影响。
    #[test]
    fn library_alias_suffix_snapshot() -> color_eyre::Result<()> {
        use mineral_model::{PlaylistId, SourceKind};

        let mut t = Terminal::new(TestBackend::new(80, 12))?;
        let mut state = crate::test_support::state_with_tracks()?;
        // 真实译名样本:迷星叫 / Mayoiuta(整首替换成 aliased_song,别名后缀在 title 列内可见)。
        if let Some(v) = state
            .library
            .tracks
            .get_mut(&PlaylistId::new(SourceKind::NETEASE, "p1"))
            .and_then(|views| views.get_mut(2))
        {
            v.data = mineral_test::aliased_song();
        }
        draw_lib(&mut t, &state)?;
        crate::test_support::assert_snap!(
            "曲目列表:歌名带译名别名,title 后缀暗色 (alias)",
            t.backend()
        );
        Ok(())
    }

    /// 别名作独立字段可搜:搜一个只出现在某曲 `alias`(歌名/艺人/专辑都不含)的词,
    /// 该曲应命中并留下,其余被滤掉——回归「展示了 alias 却搜不到、搜它反被过滤消失」。
    #[test]
    fn alias_is_searchable_as_separate_field() -> color_eyre::Result<()> {
        use mineral_model::{PlaylistId, SourceKind};

        let mut state = crate::test_support::state_with_tracks()?;
        // 把第 3 首换成真实的 迷星叫 / 别名 Mayoiuta;搜别名 "Mayoiuta"(英文名/艺人都不含)。
        if let Some(v) = state
            .library
            .tracks
            .get_mut(&PlaylistId::new(SourceKind::NETEASE, "p1"))
            .and_then(|views| views.get_mut(2))
        {
            v.data = mineral_test::aliased_song();
        }
        state.browse.search.set_query("Mayoiuta");
        let filtered = state.filtered_tracks();
        assert!(
            filtered
                .iter()
                .any(|sv| sv.data.alias.as_deref() == Some("Mayoiuta")),
            "搜别名应命中该曲"
        );
        assert!(
            filtered
                .iter()
                .all(|sv| sv.data.alias.as_deref() == Some("Mayoiuta")),
            "只有别名命中的曲应留下(歌名/艺人/专辑都不含该词)"
        );
        Ok(())
    }

    /// 别名命中与主字段同款高亮:命中子串换 search_hit 色 + 字体效果,括号与未命中
    /// 别名字符保持 overlay 暗调。命中效果只在非选中行落地(选中行整行 fg 被
    /// row_highlight 的 accent 顶掉,见 render_to 注),故把选中放在第二行、别名命中行
    /// 留在第一行。只扫 body 行(y=0 的标题栏 query badge 也染 search_hit 色,须排除):
    /// body 里 search_hit 色的字符恰好拼成 "Mayo",其余别名字符仍是 overlay。
    #[test]
    fn alias_hits_use_primary_highlight() -> color_eyre::Result<()> {
        use mineral_model::{PlaylistId, SourceKind};

        use crate::test_support::{song, with_alias, with_name};

        let theme = Theme::default();
        let mut state = crate::test_support::state_with_tracks()?;
        // 前两首各带别名 Mayoiuta(歌名各异、都不含 "mayo"),搜 "mayo" 二者皆命中。
        if let Some(views) = state
            .library
            .tracks
            .get_mut(&PlaylistId::new(SourceKind::NETEASE, "p1"))
        {
            if let Some(v) = views.get_mut(0) {
                v.data = with_alias(with_name(song("s0"), "迷星叫"), "Mayoiuta");
            }
            if let Some(v) = views.get_mut(1) {
                v.data = with_alias(with_name(song("s1"), "叫喊迷星"), "Mayoiuta");
            }
        }
        state.browse.nav.track.set_sel(1);
        state.browse.search.set_query("mayo");

        let mut t = Terminal::new(TestBackend::new(80, 12))?;
        draw_lib(&mut t, &state)?;

        let buf = t.backend().buffer();
        let (w, h) = (buf.area.width, buf.area.height);
        // 跳过 y=0(标题栏 + query badge)与 y=1(表头),只看曲目 body 行。
        let body = (2..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter_map(|(x, y)| buf.cell((x, y)))
            .collect::<Vec<_>>();
        let hit_chars = body
            .iter()
            .copied()
            .filter(|c| c.fg == theme.search_hit_color)
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert_eq!(hit_chars, "Mayo", "别名命中换 search_hit 色,与主字段同款");
        assert!(
            body.iter().any(|c| c.fg == theme.search_hit_color
                && c.modifier.contains(theme.search_hit_modifier)),
            "命中段还应叠 search_hit 字体效果"
        );
        // 未命中的别名字符与括号保持 overlay 暗调:非选中行应能扫出 "(" 与 "iuta" 的残段。
        let dim_chars = body
            .iter()
            .copied()
            .filter(|c| c.fg == theme.overlay)
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(
            dim_chars.contains("iuta)"),
            "未命中别名字符与括号应保持 overlay 暗调,实际: {dim_chars:?}"
        );
        Ok(())
    }

    /// 选中歌单但曲目未到(library.tracks 空)→ loading 态。
    #[test]
    fn library_loading_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(80, 12))?;
        let mut state = crate::test_support::state_with_playlists()?;
        state.browse.view.switch_to(View::Library);
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
        state.browse.search.set_query("zzz");
        draw_lib(&mut t, &state)?;
        crate::test_support::assert_snap!("曲目列表:搜索零命中(表内「无匹配」占位行)", t.backend());
        Ok(())
    }

    /// 聚合面阈值抬到 68:56~67 格插不起 11 格 source 列,退窄档(full=false)让序号染色兜底;
    /// 普通面仍 56 格进宽档,聚合面 68 格进宽档。
    #[test]
    fn aggregate_layout_threshold_leaves_room_for_source_column() {
        assert!(
            !TrackLayout::new(60, /*aggregate*/ true).full,
            "聚合面 60 格退窄档(插不起 source 列)"
        );
        assert!(
            TrackLayout::new(56, /*aggregate*/ false).full,
            "普通面 56 格进宽档"
        );
        assert!(
            TrackLayout::new(68, /*aggregate*/ true).full,
            "聚合面 68 格进宽档"
        );
    }

    /// 混源歌单(聚合收藏):Full 档在 album 与 len 之间多出 source 徽标列,
    /// 每行显示歌曲自己的真实来源;同源歌单(其余快照)无此列。
    #[test]
    fn library_mixed_source_column_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(80, 12))?;
        let state = crate::test_support::state_with_mixed_tracks()?;
        draw_lib(&mut t, &state)?;
        crate::test_support::assert_snap!(
            "曲目列表:混源歌单 Full 档插 per-song source 徽标列",
            t.backend()
        );
        Ok(())
    }

    /// 混源歌单窄档(Song 档):source 列插不起,序号改染该行歌曲的源色
    /// (与 queue 同一手法);同源歌单序号保持中立灰。
    #[test]
    fn library_mixed_source_narrow_tints_index() -> color_eyre::Result<()> {
        use mineral_model::SourceKind;

        use crate::render::theme::resolve_source_color;

        let theme = Theme::default();
        let state = crate::test_support::state_with_mixed_tracks()?;
        let mut t = Terminal::new(TestBackend::new(50, 12))?;
        draw_lib(&mut t, &state)?;
        // body 行 y=2 起,序号列字符 '0'/'1'/'2';逐行找到序号 cell 断言前景色。
        let fg_of = |y: u16, ch: &str| -> Option<ratatui::style::Color> {
            let buf = t.backend().buffer();
            (0..buf.area.width)
                .find_map(|x| buf.cell((x, y)).filter(|c| c.symbol() == ch).map(|c| c.fg))
        };
        // 行 0(netease)是选中行:row_highlight_style 盖掉 cell 级前景(accent),
        // 序号源色暂不可见——与设计一致,故断言非选中的行 1 / 2。
        assert_eq!(
            fg_of(2, "0"),
            Some(theme.accent),
            "选中行序号被高亮前景覆盖"
        );
        let bilibili = resolve_source_color(&theme, state.cfg.sources(), SourceKind::BILIBILI);
        assert_eq!(
            fg_of(3, "1"),
            Some(bilibili),
            "bilibili 行序号染 bilibili 色"
        );
        assert_eq!(
            fg_of(4, "2"),
            Some(theme.subtext),
            "local 未配置色,退中立兜底"
        );
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
