//! Playlists 视图渲染。

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table, Widget};

use super::badge::search_badge;
use crate::components::layout::shared::highlight::{alias_suffix, highlight_indices};
use crate::components::layout::shared::list_minimap::{MinimapCursor, render_minimap};
use crate::components::layout::shared::marquee::resolve_column_rects;
use crate::components::layout::shared::scroll_table::render_scroll_table;
use crate::components::layout::shared::text::display_width;
use crate::components::layout::shared::thumbnails::{
    THUMBNAIL_COLUMNS, render_table_thumbnails, thumbnail_phase,
};
use crate::render::theme::Theme;
use crate::runtime::deep_search::HitField;
use crate::runtime::scroll::list::ScrollMotion;
use crate::runtime::state::{AppState, ListRowIdentity, View};
use crate::runtime::view_model::PlaylistView;

/// Table 选中符；列矩形求解使用同一显示宽度。
const HIGHLIGHT_SYMBOL: &str = "▌ ";

/// 渲染 Playlists 视图到给定 [`Buffer`](正常渲染与离屏过渡合成共用此入口)。
pub fn render_to(buf: &mut Buffer, area: Rect, state: &AppState, theme: &Theme) {
    let surface = super::expansion::begin_list(buf, area, state, View::Playlists);
    let rows_data = state.filtered_playlists();
    let total = rows_data.len();
    let pos = position_label(state.browse.nav.playlist.sel(), total);

    let mut title_spans = vec![Span::styled(" playlists ", Style::new().fg(theme.subtext))];
    title_spans.extend(search_badge(
        &state.browse.search.playlists,
        super::badge::indexing_count(state),
        theme,
    ));

    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(title_spans))
        .title_bottom(Line::from(pos).style(Style::new().fg(theme.overlay)));

    // 页面形变与 view sweep 冻结视口和 minimap 光标，避免离屏绘制推进导航状态。
    let motion = if state.browse.fullscreen.at_min()
        && state.channel_search.active.at_min()
        && (state.browse.view.at_min() || state.browse.view.at_max())
    {
        ScrollMotion::Advancing {
            scrolloff: state.scrolloff(),
            glide_ticks: state.list_glide_ticks(),
        }
    } else {
        ScrollMotion::Frozen
    };

    // 全空 + 无搜索词:走 empty-state 提示分支(loading / 未登录二选一)。
    // 区分依据是 tasks_running:有任务在跑就是 loading,没任务就大概率是
    // 没登录任何源 / 各源都无歌单 —— 给出登录引导。
    if state.library.playlists.is_empty() && state.browse.search.playlists.query().is_empty() {
        paint_empty_state(buf, area, state, theme, block);
        paint_minimap(buf, area, state, theme, total, motion);
        return;
    }

    // 有词但零命中:给居中提示而非纯空白。深度索引还在飞时说「索引中」——
    // 此刻搜不到 ≠ 真没有,数据到齐后结果可能变。
    if total == 0 && !state.browse.search.playlists.query().is_empty() {
        paint_no_match(buf, area, state, theme, block);
        paint_minimap(buf, area, state, theme, total, motion);
        return;
    }

    // 确有深度命中时多一列「match」展示歌单内命中歌曲;纯歌单名命中 / 空 query
    // 不占位,不挤压 name 列宽。
    let show_match = state.has_deep_hits();
    let show_cover = state.images.supports_thumbnails();
    let mut header_cells = Vec::<Cell<'_>>::new();
    if show_cover {
        header_cells.push(Cell::from(""));
    }
    header_cells.push(Cell::from("name"));
    if show_match {
        header_cells.push(Cell::from("match"));
    }
    header_cells.extend([
        Cell::from("source"),
        Cell::from("length"),
        Cell::from("items"),
    ]);
    let header =
        Row::new(header_cells).style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));

    // name 列用 Fill 取「剩余空间」而非 Min:Min 在有 slack 时会给 ratatui 列宽求解器
    // 留多解(name>=12 + 总宽等式欠定),解不唯一 → 列宽随机差 1、帧间闪烁;Fill(1)
    // 是 name = 总宽 - 其余定宽列,唯一解,确定性。
    let mut widths = Vec::<Constraint>::new();
    if show_cover {
        widths.push(Constraint::Length(THUMBNAIL_COLUMNS));
    }
    widths.push(Constraint::Fill(1));
    if show_match {
        widths.push(Constraint::Fill(1));
    }
    widths.extend([
        Constraint::Length(11),
        Constraint::Length(8),
        Constraint::Length(5),
    ]);

    let columns = resolve_column_rects(block.inner(area), &widths, display_width(HIGHLIGHT_SYMBOL));
    let build_table = |visible: std::ops::Range<usize>| {
        let rows = visible
            .filter_map(|index| rows_data.get(index))
            .map(|p| build_row(p, state, theme, show_match, show_cover));
        Table::new(rows, widths)
            .header(header)
            .block(block)
            .row_highlight_style(
                Style::new()
                    .bg(theme.surface0)
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(HIGHLIGHT_SYMBOL)
    };

    // 视口行数 = 面板高 - 上下边框 - 表头;offset 跨帧持久(nvim 手感),滚动经缓动平移。
    // 全屏 morph 瞬态布局冻结视口，并保留空封面列。
    let viewport = usize::from(area.height.saturating_sub(3));
    let visible = render_scroll_table(
        buf,
        area,
        build_table,
        &state.browse.nav.playlist,
        total,
        viewport,
        motion,
    );
    if show_cover && let Some(column) = columns.first() {
        let covers = visible
            .clone()
            .map(|index| {
                rows_data.get(index).and_then(|playlist| {
                    crate::image::collage::effective_cover_url(state, &playlist.data)
                })
            })
            .collect::<Vec<_>>();
        render_table_thumbnails(
            buf,
            &state.images,
            *column,
            covers.iter().map(Option::as_ref),
            thumbnail_phase(state, motion, state.browse.nav.last_sel_change),
        );
    }
    paint_minimap(buf, area, state, theme, total, motion);
    super::expansion::finish_list(
        buf,
        state,
        theme,
        surface,
        visible.clone(),
        visible
            .filter_map(|index| rows_data.get(index))
            .map(|playlist| ListRowIdentity::Playlist(playlist.data.id.clone())),
    );
}

/// 在面板右边框画全列表位置：歌单只有光标，没有喜欢 / 在播标记。
///
/// # Params:
///   - `total`: 当前过滤视图的歌单总数；为零时只留轨道（空态 / 零命中同样有轨道）。
///   - `motion`: 视口滚动态，与列表导航共用，决定光标位置是否缓动。
fn paint_minimap(
    buf: &mut Buffer,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    total: usize,
    motion: ScrollMotion,
) {
    let cursor = MinimapCursor::new(
        &state.browse.nav.playlist,
        total,
        motion,
        state.minimap_cursor_ticks(),
        state.cfg.tui().minimap(),
    );
    render_minimap(
        buf,
        Rect::new(
            area.right().saturating_sub(1),
            area.y.saturating_add(1),
            area.width.min(1),
            area.height.saturating_sub(2),
        ),
        total,
        cursor,
        std::iter::empty(),
        theme,
    );
}

/// 把一个歌单组装成 sidebar 表格行(名字 [/ 深度命中] / 来源 / 总时长 / 曲目数)。
fn build_row<'a>(
    p: &'a PlaylistView,
    state: &AppState,
    theme: &Theme,
    show_match: bool,
    show_cover: bool,
) -> Row<'a> {
    let total_ms = state.total_duration_ms_of(&p.data.id);
    let len_label = if total_ms == 0 {
        String::from("—")
    } else {
        let total_min = total_ms / 60_000;
        let h = total_min / 60;
        let m = total_min % 60;
        if h == 0 {
            format!("{m}m")
        } else {
            format!("{h}h {m:02}m")
        }
    };
    let count_label = format!("{}", p.data.track_count);
    let src = p.data.source();

    let name_hits = state
        .browse
        .search
        .playlists
        .match_for(&p.data.name)
        .map(|m| m.hits);
    let mut cells = Vec::<Cell<'_>>::new();
    if show_cover {
        cells.push(Cell::from(""));
    }
    cells.push(Cell::from(Line::from(highlight_indices(
        &p.data.name,
        name_hits.as_deref().unwrap_or(&[]),
        Style::new().fg(theme.text),
        theme,
    ))));
    if show_match {
        cells.push(deep_hit_cell(p, state, theme));
    }
    cells.extend([
        Cell::from(Span::styled(
            src.label(),
            Style::new().fg(crate::render::theme::resolve_source_color(
                theme,
                state.cfg.sources(),
                src,
            )),
        )),
        Cell::from(Span::styled(len_label, Style::new().fg(theme.subtext))),
        Cell::from(Span::styled(count_label, Style::new().fg(theme.overlay))),
    ]);
    Row::new(cells)
}

/// 深度命中列:`♪ 歌名[ (别名)][ · 艺人/专辑]` + `+n` 计数;该歌单无歌曲命中时为空白格。
///
/// 各段样式与歌单内曲目行一致:别名括注 dim、命中字符同款 search_hit 高亮——
/// 命中下标按 [`HitField`] 派给对应段,其余段不带 hits。
fn deep_hit_cell<'a>(p: &PlaylistView, state: &AppState, theme: &Theme) -> Cell<'a> {
    let Some(hit) = state.deep_hit_for(&p.data.id) else {
        return Cell::from("");
    };
    let base = Style::new().fg(theme.subtext);
    let empty: &[u32] = &[];
    let (name_hits, alias_hits, second_hits) = match hit.hit_field {
        HitField::Name => (hit.hits.as_slice(), empty, empty),
        HitField::Alias => (empty, hit.hits.as_slice(), empty),
        HitField::Second => (empty, empty, hit.hits.as_slice()),
    };
    let mut spans = vec![Span::styled("♪ ", base)];
    spans.extend(highlight_indices(&hit.name, name_hits, base, theme));
    if let Some(alias) = hit.alias.as_deref() {
        spans.extend(alias_suffix(alias, alias_hits, theme));
    }
    if let Some(second) = hit.second.as_deref() {
        spans.push(Span::styled(" · ", base));
        spans.extend(highlight_indices(second, second_hits, base, theme));
    }
    if hit.extra > 0 {
        spans.push(Span::styled(
            format!(" +{}", hit.extra),
            Style::new().fg(theme.overlay),
        ));
    }
    Cell::from(Line::from(spans))
}

/// 搜索零命中时画 block + 居中提示;深度索引在飞时改提示「索引中」。
fn paint_no_match(buf: &mut Buffer, area: Rect, state: &AppState, theme: &Theme, block: Block<'_>) {
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let text = if let Some(n) = super::badge::indexing_count(state) {
        format!("indexing({n} playlist{})…", if n < 2 { "" } else { "s" })
    } else {
        "no match found".to_owned()
    };
    Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(text, Style::new().fg(theme.overlay))),
    ])
    .alignment(ratatui::layout::Alignment::Center)
    .render(inner, buf);
}

/// 全空 playlist 时画 block + 居中两行提示。loading / 未登录文案二选一。
fn paint_empty_state(
    buf: &mut Buffer,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    block: Block<'_>,
) {
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let lines: Vec<Line<'_>> = if state.tasks_snapshot.running > 0 {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "loading playlists…",
                Style::new().fg(theme.subtext),
            )),
        ]
    } else {
        // 空 = 所有源都没产出(未登录 / 该源无歌单)。不写死单源:列一个登录示例,
        // `例如` 体现多源可扩展;命令是完整子命令链(bin=mineral)。
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "还没有可用歌单",
                Style::new().fg(theme.subtext),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "登录一个音乐源,例如:",
                Style::new().fg(theme.overlay),
            )),
            Line::from(Span::styled(
                "  mineral channel netease login",
                Style::new().fg(theme.peach).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "登录后重启 mineral 生效",
                Style::new().fg(theme.overlay),
            )),
        ]
    };
    Paragraph::new(lines)
        .alignment(ratatui::layout::Alignment::Center)
        .render(inner, buf);
}

/// 拼 ` n / total ` 的 footer 标签;空列表显示 `0 / 0`。
fn position_label(sel: usize, total: usize) -> String {
    if total == 0 {
        " 0 / 0 ".to_owned()
    } else {
        format!(" {} / {total} ", sel.saturating_add(1).min(total))
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::position_label;
    use crate::runtime::state::AppState;

    /// `position_label`:1-based 当前位 / 总数;空列表 `0 / 0`;越界 clamp。
    #[test]
    fn position_label_cases() {
        assert_eq!(position_label(0, 0), " 0 / 0 ");
        assert_eq!(position_label(0, 3), " 1 / 3 ");
        assert_eq!(position_label(2, 3), " 3 / 3 ");
        assert_eq!(position_label(9, 3), " 3 / 3 ");
    }

    /// 歌单列表的右边框走同一条盲文轨道：表头行到底行都在，且只有光标没有标记。
    #[test]
    fn playlists_minimap_shows_only_the_cursor() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        let mut state = crate::test_support::state_with_playlists()?;
        state.browse.nav.playlist.place(/*sel*/ 0, /*scroll*/ 0);
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &theme);
        })?;
        let buf = t.backend().buffer();
        let symbol = |y: u16| buf.cell((39, y)).map(ratatui::buffer::Cell::symbol);
        assert_eq!(symbol(1), Some("⢹"), "光标落在轨道首行（表头行）");
        let cursor_cells = (1..=10)
            .filter(|&y| symbol(y).is_some_and(|s| matches!(s, "⢹" | "⢺" | "⢼" | "⣸")))
            .count();
        assert_eq!(cursor_cells, 1, "歌单只画光标，没有喜欢 / 在播标记");
        assert!(symbol(2).is_some_and(|s| s == "⢸"), "其余行是纯轨道");
        assert_eq!(symbol(11), Some("╯"), "底边圆角与计数不受轨道影响");
        Ok(())
    }

    /// 3 个混源歌单列表；name 列使用 `Fill(1)`，列宽由可用区域确定。
    #[test]
    fn playlists_list_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        let state = crate::test_support::state_with_playlists()?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &theme);
        })?;
        crate::test_support::assert_snap!(
            "歌单列表:3 个混源歌单(EndSerenading / 本地)",
            t.backend()
        );
        Ok(())
    }

    /// Kitty 封面保留选中行样式与一格名称间距；缺图不移位，其他协议维持旧文本位置。
    #[test]
    fn thumbnails_preserve_selected_row_and_text_spacing() -> color_eyre::Result<()> {
        use std::sync::Arc;

        use mineral_model::MediaUrl;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::style::{Modifier, Style};

        use crate::image::{ImageEngine, ImageRenderPhase};

        let theme = crate::test_support::default_theme()?;
        let mut state = crate::test_support::state_with_playlists()?;
        let url = MediaUrl::remote("https://example.com/playlist-cover.png")?;
        state
            .library
            .playlists
            .first_mut()
            .ok_or_else(|| color_eyre::eyre::eyre!("缺少测试歌单"))?
            .data
            .cover_url = Some(url.clone());
        let area = Rect::new(4, 3, 64, 8);
        let mut text = Buffer::empty(area);
        super::render_to(&mut text, area, &state, &theme);
        let first_row = area.y + 2;
        let old_name_x = (area.left()..area.right())
            .find(|&x| text.cell((x, first_row)).is_some_and(|c| c.symbol() == "E"))
            .ok_or_else(|| color_eyre::eyre::eyre!("旧布局缺少歌单名称"))?;
        assert!(
            text.content
                .iter()
                .all(|c| !c.symbol().contains('\u{10EEEE}'))
        );

        state.images = ImageEngine::disabled_kitty(Arc::clone(&state.cfg));
        state.images.insert_test_thumbnail(&url)?;
        let mut expected = Buffer::empty(Rect::new(0, 0, super::THUMBNAIL_COLUMNS, 1));
        expected.set_style(
            expected.area,
            Style::new()
                .bg(theme.surface0)
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        );
        for _ in 0..2 {
            state.images.render_thumbnail(
                Some(&url),
                expected.area,
                &mut expected,
                ImageRenderPhase::Stable,
            );
        }
        let mut kitty = Buffer::empty(area);
        super::render_to(&mut kitty, area, &state, &theme);
        for column in 0..super::THUMBNAIL_COLUMNS {
            assert_eq!(
                kitty.cell((old_name_x + column, first_row)),
                expected.cell((column, 0)),
                "选中行必须在高亮后覆盖同一 Kitty 图片"
            );
        }
        let name_x = old_name_x + super::THUMBNAIL_COLUMNS + 1;
        assert!(
            kitty
                .cell((old_name_x, first_row))
                .is_some_and(|c| c.symbol().contains('\u{10EEEE}'))
        );
        assert_eq!(
            kitty
                .cell((name_x - 1, first_row))
                .map(ratatui::buffer::Cell::symbol),
            Some(" ")
        );
        assert_eq!(
            kitty
                .cell((name_x, first_row))
                .map(ratatui::buffer::Cell::symbol),
            Some("E")
        );
        assert_eq!(
            kitty
                .cell((old_name_x, first_row + 1))
                .map(ratatui::buffer::Cell::symbol),
            Some(" "),
            "没有封面的下一行保留空图片格"
        );
        assert_eq!(
            kitty
                .cell((name_x, first_row + 1))
                .map(ratatui::buffer::Cell::symbol),
            Some("T")
        );
        assert_eq!(
            kitty
                .cell((old_name_x, first_row - 1))
                .map(ratatui::buffer::Cell::symbol),
            Some(" "),
            "图片列表头为空"
        );
        assert_eq!(
            kitty.cell((name_x, first_row)).map(|c| (c.fg, c.bg)),
            Some((theme.accent, theme.surface0)),
            "名称保留整行高亮"
        );
        Ok(())
    }

    /// 搜索输入态:标题挂 `/查询█`(末尾光标方块,表示正在输入)。
    #[test]
    fn playlists_search_active_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        let mut state = crate::test_support::state_with_playlists()?;
        state.browse.search.playlists.typing = true;
        state.browse.search.playlists.set_query("春日影");
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &theme);
        })?;
        crate::test_support::assert_snap!("歌单列表:搜索输入态(标题 /春日影█)", t.backend());
        Ok(())
    }

    /// 拼音首字母搜索:输入 `cry` → 命中「春日影」,Han 三字均高亮(反向映射)。
    #[test]
    fn playlists_search_pinyin_initials_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        use mineral_model::SourceKind;

        let mut state = AppState::test_default()?;
        state.library.playlists = vec![
            crate::test_support::playlist_view("a", "MyGO!!!!!", SourceKind::NETEASE, 1),
            crate::test_support::playlist_view("b", "春日影", SourceKind::NETEASE, 1),
        ];
        state.browse.search.playlists.set_query("cry");
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &theme);
        })?;
        crate::test_support::assert_snap!(
            "歌单列表:首字母 cry 命中「春日影」(汉字三字均高亮)",
            t.backend()
        );
        Ok(())
    }

    /// 全拼搜索:输入 `chunying` → 命中「春日影」,春 + 影 高亮(日的 ri 未命中)。
    #[test]
    fn playlists_search_pinyin_full_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        use mineral_model::SourceKind;

        let mut state = AppState::test_default()?;
        state.library.playlists = vec![crate::test_support::playlist_view(
            "a",
            "春日影",
            SourceKind::NETEASE,
            1,
        )];
        state.browse.search.playlists.set_query("chunying");
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &theme);
        })?;
        crate::test_support::assert_snap!(
            "歌单列表:全拼 chunying 命中「春日影」(春 + 影 高亮)",
            t.backend()
        );
        Ok(())
    }

    /// 深度命中:歌单名均不含搜索词,p2 内「春日影 · CRYCHIC」命中 → 该歌单被捞出,
    /// match 列展示 `♪ 春日影 · CRYCHIC`(命中字符同款高亮)。
    #[test]
    fn playlists_search_deep_hit_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        use mineral_model::{PlaylistId, SourceKind};

        use crate::test_support::{entry_views, song, with_artist, with_name};

        let mut state = crate::test_support::state_with_playlists()?;
        let track = with_artist(with_name(song("s1"), "春日影"), "CRYCHIC");
        state.library.tracks.insert(
            PlaylistId::new(SourceKind::NETEASE, "p2"),
            crate::runtime::state::PlaylistTracks {
                entries: entry_views(vec![track]),
                complete: true,
                next_offset: None,
            },
        );
        state.library.tracks_generation = 1;
        state.browse.search.playlists.set_query("春日");

        let mut t = Terminal::new(TestBackend::new(64, 12))?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &theme);
        })?;
        crate::test_support::assert_snap!(
            "歌单列表:深度命中(match 列展示 ♪ 春日影 · CRYCHIC,命中高亮)",
            t.backend()
        );
        Ok(())
    }

    /// 深度命中落在别名:别名以括注紧跟歌名(dim,与曲目行样式一致),次段仍是首位艺人——
    /// match 列展示 `♪ 迷星叫 (Mayoiuta) · MyGO!!!!!`。
    #[test]
    fn playlists_search_deep_hit_alias_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        use mineral_model::{PlaylistId, SourceKind};

        use crate::test_support::{entry_views, song, with_alias, with_artist, with_name};

        let mut state = crate::test_support::state_with_playlists()?;
        let track = with_alias(
            with_artist(with_name(song("s1"), "迷星叫"), "MyGO!!!!!"),
            "Mayoiuta",
        );
        state.library.tracks.insert(
            PlaylistId::new(SourceKind::NETEASE, "p2"),
            crate::runtime::state::PlaylistTracks {
                entries: entry_views(vec![track]),
                complete: true,
                next_offset: None,
            },
        );
        state.library.tracks_generation = 1;
        state.browse.search.playlists.set_query("mayo");

        let mut t = Terminal::new(TestBackend::new(100, 12))?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &theme);
        })?;
        crate::test_support::assert_snap!(
            "歌单列表:别名深度命中(match 列 ♪ 迷星叫 (Mayoiuta) · MyGO!!!!!,别名括注 dim)",
            t.backend()
        );
        Ok(())
    }

    /// 搜索零命中(无在飞索引):居中「无匹配」提示而非纯空白。
    #[test]
    fn playlists_search_no_match_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut state = crate::test_support::state_with_playlists()?;
        state.browse.search.playlists.set_query("zzz");
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &theme);
        })?;
        crate::test_support::assert_snap!("歌单列表:搜索零命中(居中「无匹配」)", t.backend());
        Ok(())
    }

    /// 搜索零命中 + 深度索引在飞:badge 缀 ⟳n,居中提示「索引中」——
    /// 此刻搜不到 ≠ 真没有。
    #[test]
    fn playlists_search_indexing_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut state = crate::test_support::state_with_playlists()?;
        state.browse.search.playlists.set_query("zzz");
        let ids = state
            .library
            .playlists
            .iter()
            .map(|playlist| playlist.data.id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            state
                .library
                .request_playlist(id, mineral_channel_core::PlaylistLoad::Complete);
        }
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &theme);
        })?;
        crate::test_support::assert_snap!(
            "歌单列表:零命中且索引在飞(badge ⟳3 + 居中「索引中」)",
            t.backend()
        );
        Ok(())
    }

    /// 空列表(尚未加载 / 未登录)。
    #[test]
    fn playlists_empty_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        let state = AppState::test_default()?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &theme);
        })?;
        crate::test_support::assert_snap!("歌单列表:空态(未加载 / 未登录)", t.backend());
        Ok(())
    }
}
