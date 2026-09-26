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
