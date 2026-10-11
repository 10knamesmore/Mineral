//! 当前类型的项目列表；列名固定，抽屉只占用左侧空间。

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, HighlightSpacing, Paragraph, Row, StatefulWidget, Table,
    TableState, Widget,
};

use super::super::badge::search_badge;
use super::super::preparation::minimap_track;
use super::view::CollectionView as CollectionFrame;
use super::window::CollectionWindow;
use super::{CollectionInput, CollectionList};
use crate::components::frame::PrepareCx;
use crate::components::layout::shared::highlight::{alias_suffix, highlight_indices};
use crate::components::layout::shared::list_expansion;
use crate::components::layout::shared::list_minimap::{MinimapCursor, prepare_minimap};
use crate::components::layout::shared::marquee::resolve_column_rects;
use crate::components::layout::shared::scroll_table::{PreparedMinimap, PreparedTable};
use crate::components::layout::shared::text::display_width;
use crate::components::layout::shared::thumbnails::THUMBNAIL_COLUMNS;
use crate::components::lifecycle::Prepare;
use crate::render::theme::{Theme, resolve_source_color};
use crate::runtime::collection::{CollectionGroup, CollectionRef};
use crate::runtime::deep_search::HitField;
use crate::runtime::scroll::list::ScrollMotion;
use crate::runtime::state::FilteredListFrame;

/// 列几何和表格绘制使用相同光标宽度。
const HIGHLIGHT_SYMBOL: &str = "▌ ";

/// 挤压后优先保留名称，较宽时补充艺人和来源等信息。
struct Columns {
    /// 协议支持且可容纳行内封面。
    cover: bool,

    /// 专辑艺人列。
    artist: bool,

    /// 深度搜索命中的曲目。
    matched: bool,

    /// 音乐来源列。
    source: bool,

    /// 已知总时长。
    length: bool,

    /// 项目曲目数。
    count: bool,
}

impl Columns {
    /// 图片、表头和行内容共用宽度判断。
    fn new(view: &CollectionFrame<'_>, width: u16) -> Self {
        Self {
            cover: view.images.supports_thumbnails() && width >= 20,
            artist: view.list.group() == CollectionGroup::Albums && width >= 68,
            matched: view.list.has_deep_hits() && width >= 60,
            source: width >= 44,
            length: width >= 55,
            count: width >= 26,
        }
    }

    /// 文本列分配剩余空间，数值列保持可读宽度。
    fn widths(&self) -> Vec<Constraint> {
        let mut widths = Vec::new();
        if self.cover {
            widths.push(Constraint::Length(THUMBNAIL_COLUMNS));
        }
        widths.push(Constraint::Fill(3));
        if self.artist {
            widths.push(Constraint::Fill(2));
        }
        if self.matched {
            widths.push(Constraint::Fill(2));
        }
        if self.source {
            widths.push(Constraint::Length(11));
        }
        if self.length {
            widths.push(Constraint::Length(8));
        }
        if self.count {
            widths.push(Constraint::Length(5));
        }
        widths
    }

    /// 固定列名不属于项目行序列。
    fn header(&self, theme: &Theme) -> Row<'static> {
        let mut cells = Vec::new();
        if self.cover {
            cells.push(Cell::from(""));
        }
        cells.push(Cell::from("name"));
        if self.artist {
            cells.push(Cell::from("artist"));
        }
        if self.matched {
            cells.push(Cell::from("match"));
        }
        if self.source {
            cells.push(Cell::from("source"));
        }
        if self.length {
            cells.push(Cell::from("length"));
        }
        if self.count {
            cells.push(Cell::from("items"));
        }
        Row::new(cells).style(Style::new().fg(theme.overlay).bg(theme.mantle))
    }
}

impl Prepare for CollectionList {
    type Input<'a> = CollectionInput<'a>;

    /// 抽屉的实时宽度同时用于光标、图片和筛选动画。
    fn prepare(&mut self, area: Rect, input: CollectionInput<'_>, cx: &mut PrepareCx<'_>) {
        let area = self.content_area(area);
        let frame = cx.frame;
        let motion = cx.motion;
        self.active_mut().stable = matches!(motion, ScrollMotion::Advancing { .. });
        let rows = self.rows(input.library, frame.config);
        let total = rows.len();
        self.active_mut().scroll.prepare(
            total,
            CollectionWindow::scroll_height(area),
            motion,
            frame.cursor_ticks(),
            cx.advance,
        );
        let window = CollectionWindow::new(area, &rows, self.scroll());
        let view = self.view(input, frame, cx.images.ready());
        if let Some(column) = thumbnail_column(&view, &window) {
            for (index, row) in rows
                .iter()
                .enumerate()
                .skip(window.visible.start)
                .take(window.visible.len())
            {
                if let Some(row_area) = window.row_area(index) {
                    let cover = view.cover(row);
                    cx.images.thumbnail(
                        cover.as_ref(),
                        Rect::new(column.x, row_area.y, column.width, 1),
                        cx.image_phase,
                    );
                }
            }
        }
        prepare_minimap(
            &mut self.active_mut().scroll,
            total,
            minimap_track(area),
            std::iter::empty(),
            motion,
            frame.cursor_ticks(),
            frame.config.minimap(),
            cx.advance,
        );
        if self.active().stable {
            let filtered = (!self.search().query().is_empty()).then(|| {
                let view = self.view(input, frame, cx.images.ready());
                FilteredListFrame {
                    area,
                    body: window.body,
                    content: table(area, &view, &rows, &window),
                    rows: rows
                        .iter()
                        .skip(window.visible.start)
                        .take(window.visible.len())
                        .map(|row| row.id())
                        .collect(),
                }
            });
            let expansion = &mut self.active_mut().expansion;
            expansion.prepare(area, window.body, filtered);
            expansion.declare_images(&mut cx.images);
        }
    }
}

impl CollectionFrame<'_> {
    /// 抽屉与列表在同一组件中绘制，右侧详情无需移动。
    pub(crate) fn paint(&self, area: Rect, buf: &mut Buffer) {
        let theme = self.frame.theme;
        self.list.drawer.paint(area, self.input.library, theme, buf);
        self.paint_content(self.list.content_area(area), buf);
        self.list.drawer.paint_focus_ring(area, theme, buf);
    }

    /// 空列表与筛选结果共用内容绘制，浮动焦点边框由外层最后叠画。
    fn paint_content(&self, area: Rect, buf: &mut Buffer) {
        let theme = self.frame.theme;
        let rows = self.rows();
        let window = CollectionWindow::new(area, &rows, self.list.scroll());
        let state = self.list.active();
        let active = state
            .stable
            .then_some(state.expansion.active.as_ref())
            .flatten();
        let surface = list_expansion::begin_list(buf, area, window.body, active);
        let block = frame_block(self, theme);
        if rows.is_empty() {
            if self.list.search().query().is_empty() {
                paint_empty_state(buf, area, self, theme, block);
            } else {
                paint_no_match(buf, area, self, theme, block);
            }
            return;
        }
        table(area, self, &rows, &window).paint(buf);
        list_expansion::finish_list(buf, active, theme, surface, window.visible.clone());
        let columns = Columns::new(self, window.table.width);
        StatefulWidget::render(
            Table::new([columns.header(theme)], columns.widths())
                .highlight_symbol(HIGHLIGHT_SYMBOL)
                .highlight_spacing(HighlightSpacing::Always),
            Rect::new(
                window.table.x,
                window.table.y,
                window.table.width,
                window.table.height.min(1),
            ),
            buf,
            &mut TableState::default(),
        );
    }
}

/// 当前焦点用边框区分，返回提示不占用标题。
fn frame_block(view: &CollectionFrame<'_>, theme: &Theme) -> Block<'static> {
    let color = if view.list.choosing_type() || !view.list.drawer_settled() {
        theme.surface1
    } else {
        theme.accent
    };
    let mut title = vec![Span::styled(
        format!(" {} ", view.list.group().label()),
        Style::new().fg(color),
    )];
    title.extend(search_badge(
        view.list.search(),
        view.indexing_count(),
        theme,
    ));
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .title(Line::from(title))
        .title_bottom(
            Line::from(position_label(view.list.scroll().sel(), view.rows().len()))
                .style(Style::new().fg(theme.overlay)),
        )
}

/// 列表图片与文字列使用同一光标留白。
fn column_rects(view: &CollectionFrame<'_>, window: &CollectionWindow) -> Vec<Rect> {
    resolve_column_rects(
        window.table,
        &Columns::new(view, window.table.width).widths(),
        display_width(HIGHLIGHT_SYMBOL),
    )
}

/// 不绘制被抽屉挤成不足两格的图片。
fn thumbnail_column(view: &CollectionFrame<'_>, window: &CollectionWindow) -> Option<Rect> {
    Columns::new(view, window.table.width)
        .cover
        .then(|| column_rects(view, window).first().copied())
        .flatten()
        .filter(|column| column.width == THUMBNAIL_COLUMNS)
}

/// 筛选展开保存可重绘的文本和图片位置。
fn table(
    area: Rect,
    view: &CollectionFrame<'_>,
    rows: &[CollectionRef<'_>],
    window: &CollectionWindow,
) -> PreparedTable {
    let theme = view.frame.theme;
    let columns = Columns::new(view, window.table.width);
    let table = Table::new(
        rows.iter()
            .skip(window.visible.start)
            .take(window.visible.len())
            .map(|row| build_row(*row, view, theme, &columns)),
        columns.widths(),
    )
    .header(columns.header(theme))
    .row_highlight_style(Style::new().bg(theme.surface0).add_modifier(Modifier::BOLD))
    .highlight_symbol(Span::styled(
        HIGHLIGHT_SYMBOL,
        Style::new().fg(if view.list.choosing_type() {
            theme.overlay
        } else {
            theme.accent
        }),
    ))
    .highlight_spacing(HighlightSpacing::Always);
    let mut images = Vec::new();
    if let Some(column) = thumbnail_column(view, window) {
        for (index, row) in rows
            .iter()
            .enumerate()
            .skip(window.visible.start)
            .take(window.visible.len())
        {
            if let Some(row_area) = window.row_area(index)
                && let Some(cover) = view.cover(row)
                && let Some(image) = view
                    .images
                    .inline_image(&cover, Rect::new(column.x, row_area.y, column.width, 1))
            {
                images.push(image);
            }
        }
    }
    PreparedTable {
        table,
        area: window.table,
        selected: window.selected.map(|index| index - window.visible.start),
        border: Some((area, frame_block(view, theme))),
        images,
        minimap: Some(PreparedMinimap {
            area: minimap_track(area),
            total: rows.len(),
            cursor: MinimapCursor::new(view.list.scroll(), rows.len(), view.frame.config.minimap()),
            entries: Vec::new(),
            theme: *theme,
        }),
    }
}

/// 一行始终对应一个歌单或专辑，名称与艺人分别使用真实表格列。
fn build_row(
    item: CollectionRef<'_>,
    view: &CollectionFrame<'_>,
    theme: &Theme,
    columns: &Columns,
) -> Row<'static> {
    let mut cells = Vec::new();
    if columns.cover {
        cells.push(Cell::from(""));
    }
    let hits = view
        .list
        .search()
        .match_for(item.name())
        .map(|matched| matched.hits);
    cells.push(Cell::from(Line::from(highlight_indices(
        item.name(),
        hits.as_deref().unwrap_or(&[]),
        Style::new().fg(theme.text),
        theme,
    ))));
    if columns.artist {
        let artists = item
            .artists()
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(" / ");
        let hits = view
            .list
            .search()
            .match_for(&artists)
            .map(|matched| matched.hits);
        cells.push(Cell::from(Line::from(highlight_indices(
            &artists,
            hits.as_deref().unwrap_or(&[]),
            Style::new().fg(theme.subtext),
            theme,
        ))));
    }
    if columns.matched {
        cells.push(deep_hit_cell(item, view, theme));
    }
    if columns.source {
        cells.push(Cell::from(Span::styled(
            item.source().label(),
            Style::new().fg(resolve_source_color(
                theme,
                view.frame.config.source_colors(),
                item.source(),
            )),
        )));
    }
    if columns.length {
        let length = item
            .id()
            .tracks(view.input.library)
            .map(|_| view.duration_ms(&item.id()) / 60_000)
            .map_or_else(
                || "—".to_owned(),
                |min| {
                    if min < 60 {
                        format!("{min}m")
                    } else {
                        format!("{}h {:02}m", min / 60, min % 60)
                    }
                },
            );
        cells.push(Cell::from(Span::styled(
            length,
            Style::new().fg(theme.subtext),
        )));
    }
    if columns.count {
        cells.push(Cell::from(Span::styled(
            item.track_count()
                .map_or_else(|| "—".to_owned(), |n| n.to_string()),
            Style::new().fg(theme.overlay),
        )));
    }
    Row::new(cells)
}

/// 深度命中列:`♪ 歌名[ (别名)][ · 艺人/专辑]` + `+n` 计数;该歌单无歌曲命中时为空白格。
///
/// 各段样式与歌单内曲目行一致:别名括注 dim、命中字符同款 search_hit 高亮——
/// 命中下标按 [`HitField`] 派给对应段,其余段不带 hits。
fn deep_hit_cell<'a>(p: CollectionRef<'_>, view: &CollectionFrame<'_>, theme: &Theme) -> Cell<'a> {
    let Some(hit) = view.list.deep_hit_for(p.id()) else {
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
    view: &CollectionFrame<'_>,
    theme: &Theme,
    block: Block<'_>,
) {
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let text = if let Some(n) = view.indexing_count() {
        format!("indexing({n} collection{})…", if n < 2 { "" } else { "s" })
    } else {
        "no match found".to_owned()
    };
    Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(text, Style::new().fg(theme.overlay))),
    ])
    .alignment(Alignment::Center)
    .render(inner, buf);
}

/// 全空 playlist 时画 block + 居中两行提示。loading / 未登录文案二选一。
fn paint_empty_state(
    buf: &mut Buffer,
    area: Rect,
    view: &CollectionFrame<'_>,
    theme: &Theme,
    block: Block<'_>,
) {
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let kind = view.list.group().label();
    let text = if view.input.loading {
        format!("loading {kind}…")
    } else {
        format!("no {kind} yet")
    };
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(text, Style::new().fg(theme.subtext))),
    ];
    Paragraph::new(lines)
        .alignment(Alignment::Center)
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
