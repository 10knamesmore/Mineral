//! Playlists 视图渲染。

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table, Widget};

use super::super::badge::search_badge;
use super::view::PlaylistView as PlaylistFrame;
use super::{PlaylistInput, PlaylistList};
use crate::components::frame::PrepareCx;
use crate::components::layout::shared::highlight::{alias_suffix, highlight_indices};
use crate::components::layout::shared::list_expansion;
use crate::components::layout::shared::list_minimap::{MinimapCursor, render_minimap};
use crate::components::layout::shared::marquee::resolve_column_rects;
use crate::components::layout::shared::scroll_table::render_scroll_table;
use crate::components::layout::shared::text::display_width;
use crate::components::layout::shared::thumbnails::{
    THUMBNAIL_COLUMNS, declare_table_thumbnails, render_table_thumbnails,
};
use crate::render::theme::Theme;
use crate::runtime::deep_search::HitField;
use crate::runtime::scroll::list::ScrollMotion;
use crate::runtime::view_model::PlaylistView;

/// Table 选中符；列矩形求解使用同一显示宽度。
const HIGHLIGHT_SYMBOL: &str = "▌ ";

/// 先更新列表视口，再收集同一可见窗口的缩略图需求。
impl PlaylistList {
    /// 在稳定布局中更新自己的视口并声明可见图片。
    pub(crate) fn prepare(&mut self, area: Rect, input: PlaylistInput<'_>, cx: &mut PrepareCx<'_>) {
        let frame = cx.frame;
        let theme = frame.theme;
        let advance = cx.advance;
        self.stable = matches!(cx.motion, ScrollMotion::Advancing { .. });
        let total = self.rows(input.library, frame.config).len();
        let motion = cx.motion;
        let ticks = frame.cursor_ticks();
        let viewport = usize::from(area.height.saturating_sub(3));
        self.scroll.prepare(total, viewport, motion, ticks, advance);
        let show_cover = cx.images.ready().supports_thumbnails();
        let widths = column_constraints(show_cover, self.has_deep_hits());
        let inner = Block::new().borders(Borders::ALL).inner(area);
        let columns = resolve_column_rects(inner, &widths, display_width(HIGHLIGHT_SYMBOL));
        let offset = self.scroll.offset(total, viewport);
        if show_cover && let Some(column) = columns.first() {
            let rows = self.rows(input.library, frame.config);
            let covers = (offset..offset.saturating_add(viewport).min(total))
                .map(|index| {
                    rows.get(index).and_then(|p| {
                        crate::image::collage::effective_cover_url(
                            input.library,
                            cx.images.ready(),
                            &p.data,
                        )
                    })
                })
                .collect::<Vec<_>>();
            let phase = cx.image_phase;
            declare_table_thumbnails(
                &mut cx.images,
                *column,
                covers.iter().map(Option::as_ref),
                phase,
            );
        }
        crate::components::layout::shared::list_minimap::prepare_minimap(
            &mut self.scroll,
            total,
            super::super::preparation::minimap_track(area),
            std::iter::empty(),
            motion,
            ticks,
            frame.config.tui().minimap(),
            advance,
        );
        if matches!(motion, ScrollMotion::Advancing { .. }) {
            let filtered = (!self.search.query().is_empty()).then(|| {
                use crate::components::layout::shared::scroll_table::{
                    PreparedMinimap, PreparedTable,
                };
                let visible = offset..offset.saturating_add(viewport).min(total);
                let rows_data = self.rows(input.library, frame.config);
                let rows = visible
                    .clone()
                    .filter_map(|index| rows_data.get(index))
                    .map(|p| p.data.id.clone())
                    .collect();
                let covers = visible
                    .clone()
                    .map(|index| {
                        rows_data.get(index).and_then(|p| {
                            crate::image::collage::effective_cover_url(
                                input.library,
                                cx.images.ready(),
                                &p.data,
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                let (table, columns, show_cover) = table(
                    area,
                    &self.view(input, frame, cx.images.ready()),
                    theme,
                    visible,
                );
                let images =
                    columns
                        .first()
                        .filter(|_| show_cover)
                        .map_or_else(Vec::new, |column| {
                            crate::components::layout::shared::thumbnails::snapshot_thumbnails(
                                cx.images.ready(),
                                *column,
                                covers.iter().map(Option::as_ref),
                            )
                        });
                let content = PreparedTable {
                    table,
                    area,
                    border: None,
                    images,
                    selected: (total > 0).then(|| {
                        crate::runtime::scroll::viewport::pin_cursor(
                            self.scroll.sel(),
                            offset,
                            viewport,
                        )
                        .saturating_sub(offset)
                    }),
                    minimap: Some(PreparedMinimap {
                        area: super::super::preparation::minimap_track(area),
                        total,
                        cursor: MinimapCursor::new(
                            &self.scroll,
                            total,
                            frame.config.tui().minimap(),
                        ),
                        entries: Vec::new(),
                        theme: *theme,
                    }),
                };
                crate::runtime::state::FilteredListFrame {
                    area,
                    body: super::super::expansion::body(area),
                    content,
                    rows,
                }
            });
            self.expansion
                .prepare(area, super::super::expansion::body(area), filtered);
            self.expansion.declare_images(&mut cx.images);
        }
    }
}

/// 表格和资源准备共用的列约束。
fn column_constraints(show_cover: bool, show_match: bool) -> Vec<Constraint> {
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
    widths
}

/// 渲染 Playlists 视图到给定 [`Buffer`](正常渲染与离屏过渡合成共用此入口)。
impl PlaylistFrame<'_> {
    /// 将本组件的只读输入画入字符格。
    pub(crate) fn paint(&self, area: Rect, buf: &mut Buffer) {
        let view = self;
        let theme = self.frame.theme;
        let active = view
            .list
            .stable
            .then_some(view.list.expansion.active.as_ref())
            .flatten();
        let surface =
            list_expansion::begin_list(buf, area, super::super::expansion::body(area), active);
        let rows_data = view.rows();
        let total = rows_data.len();
        let block = frame_block(view, theme, total);

        // 页面形变与 view sweep 冻结视口和 minimap 光标，避免离屏绘制推进导航状态。

        // 全空 + 无搜索词:走 empty-view 提示分支(loading / 未登录二选一)。
        // 区分依据是 tasks_running:有任务在跑就是 loading,没任务就大概率是
        // 没登录任何源 / 各源都无歌单 —— 给出登录引导。
        if view.input.library.playlists.is_empty() && view.list.search.query().is_empty() {
            paint_empty_state(buf, area, view, theme, block);
            paint_minimap(buf, area, view, theme, total);
            return;
        }

        // 有词但零命中:给居中提示而非纯空白。深度索引还在飞时说「索引中」——
        // 此刻搜不到 ≠ 真没有,数据到齐后结果可能变。
        if total == 0 && !view.list.search.query().is_empty() {
            paint_no_match(buf, area, view, theme, block);
            paint_minimap(buf, area, view, theme, total);
            return;
        }

        // 视口行数 = 面板高 - 上下边框 - 表头;offset 跨帧持久(nvim 手感),滚动经缓动平移。
        // 全屏 morph 瞬态布局冻结视口，并保留空封面列。
        let viewport = usize::from(area.height.saturating_sub(3));
        let offset = view.list.scroll.offset(total, viewport);
        let window = offset..offset.saturating_add(viewport).min(total);
        let (table, columns, show_cover) = table(area, view, theme, window);
        let visible = render_scroll_table(buf, area, |_| table, &view.list.scroll, total, viewport);
        if show_cover && let Some(column) = columns.first() {
            let covers = visible
                .clone()
                .map(|index| {
                    rows_data.get(index).and_then(|playlist| {
                        crate::image::collage::effective_cover_url(
                            view.input.library,
                            view.images,
                            &playlist.data,
                        )
                    })
                })
                .collect::<Vec<_>>();
            render_table_thumbnails(buf, view.images, *column, covers.iter().map(Option::as_ref));
        }
        paint_minimap(buf, area, view, theme, total);
        list_expansion::finish_list(buf, active, theme, surface, visible.clone());
    }
}

/// 搜索展开与当前表格共用外框元数据。
fn frame_block(view: &PlaylistFrame<'_>, theme: &Theme, total: usize) -> Block<'static> {
    let pos = position_label(view.list.scroll.sel(), total);

    let mut title_spans = vec![Span::styled(" playlists ", Style::new().fg(theme.subtext))];
    title_spans.extend(search_badge(
        &view.list.search,
        view.indexing_count(),
        theme,
    ));

    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(title_spans))
        .title_bottom(Line::from(pos).style(Style::new().fg(theme.overlay)))
}

/// 构造可见歌单的表格内容，不修改列表生命周期。
fn table(
    area: Rect,
    view: &PlaylistFrame<'_>,
    theme: &Theme,
    visible: std::ops::Range<usize>,
) -> (Table<'static>, Vec<Rect>, bool) {
    let rows_data = view.rows();
    let block = frame_block(view, theme, rows_data.len());
    // 确有深度命中时多一列「match」展示歌单内命中歌曲;纯歌单名命中 / 空 query
    // 不占位,不挤压 name 列宽。
    let show_match = view.list.has_deep_hits();
    let show_cover = view.images.supports_thumbnails();
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
    let widths = column_constraints(show_cover, show_match);

    let columns = resolve_column_rects(block.inner(area), &widths, display_width(HIGHLIGHT_SYMBOL));
    let build_table = |visible: std::ops::Range<usize>| {
        let rows = visible
            .filter_map(|index| rows_data.get(index))
            .map(|p| build_row(p, view, theme, show_match, show_cover));
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

    (build_table(visible), columns, show_cover)
}

/// 在面板右边框画全列表位置：歌单只有光标，没有喜欢 / 在播标记。
///
/// # Params:
///   - `total`: 当前过滤视图的歌单总数；为零时只留轨道（空态 / 零命中同样有轨道）。
///   - `motion`: 视口滚动态，与列表导航共用，决定光标位置是否缓动。
fn paint_minimap(
    buf: &mut Buffer,
    area: Rect,
    view: &PlaylistFrame<'_>,
    theme: &Theme,
    total: usize,
) {
    let cursor = MinimapCursor::new(&view.list.scroll, total, view.frame.config.tui().minimap());
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
fn build_row(
    p: &PlaylistView,
    view: &PlaylistFrame<'_>,
    theme: &Theme,
    show_match: bool,
    show_cover: bool,
) -> Row<'static> {
    let total_ms = view.duration_ms(&p.data.id);
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

    let name_hits = view.list.search.match_for(&p.data.name).map(|m| m.hits);
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
        cells.push(deep_hit_cell(p, view, theme));
    }
    cells.extend([
        Cell::from(Span::styled(
            src.label(),
            Style::new().fg(crate::render::theme::resolve_source_color(
                theme,
                view.frame.config.sources(),
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
fn deep_hit_cell<'a>(p: &PlaylistView, view: &PlaylistFrame<'_>, theme: &Theme) -> Cell<'a> {
    let Some(hit) = view.list.deep_hit_for(&p.data.id) else {
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
fn paint_no_match(
    buf: &mut Buffer,
    area: Rect,
    view: &PlaylistFrame<'_>,
    theme: &Theme,
    block: Block<'_>,
) {
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let text = if let Some(n) = view.indexing_count() {
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
    view: &PlaylistFrame<'_>,
    theme: &Theme,
    block: Block<'_>,
) {
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let lines: Vec<Line<'_>> = if view.input.loading {
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
