//! Library 视图渲染:展示已打开歌单内的曲目。

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Row, Table};

use mineral_model::SourceKind;

use super::super::badge::search_badge;
use super::{TrackInput, TrackList, TrackView};
use crate::components::frame::PrepareCx;
use crate::components::layout::shared::highlight::{alias_suffix, highlight_indices};
use crate::components::layout::shared::list_expansion;
use crate::components::layout::shared::list_minimap::{
    MinimapCursor, MinimapEntry, render_minimap,
};
use crate::components::layout::shared::marquee::{
    MarqueeCtx, RowMarquee, resolve_column_rects, row_marquee,
};
use crate::components::layout::shared::scroll_table::render_scroll_table;
use crate::components::layout::shared::text::display_width;
use crate::components::layout::shared::thumbnails::{
    THUMBNAIL_COLUMNS, declare_table_thumbnails, render_table_thumbnails,
};
use crate::render::theme::Theme;
use crate::runtime::format::format_ms_opt;
use crate::runtime::scroll::list::ScrollMotion;
use crate::runtime::view_model::PlaylistEntryView;

/// Table 选中符；列矩形求解使用同一显示宽度。
const HIGHLIGHT_SYMBOL: &str = "▌ ";

/// 按面板宽度、歌单来源与图片协议选择曲目表列集。
#[derive(Clone, Copy)]
struct TrackLayout {
    /// 宽档:♥ / title / artist / album / len,文本列比例 Fill(3:2:2);
    /// `false` 为窄档 ♥ / title / len，省去 artist/album。
    full: bool,

    /// 聚合面(source = mineral 的跨源歌单,如全源收藏合集):宽档在 album 后插
    /// 每首歌的 source 徽标列，窄档省去。普通单源歌单为 `false`。
    aggregate: bool,

    /// 当前协议支持 Kitty 缩略图时，在 title 前保留图片列。
    thumbnails: bool,
}

impl TrackLayout {
    /// 按面板宽度与曲目集合选布局。普通面 56 格起显示 artist/album；
    /// 聚合面还需 11 格 source 列及间隔，68 格起使用宽档。
    ///
    /// # Params:
    ///   - `width`: 含边框的面板宽度，单位 cell
    ///   - `aggregate`: 歌单是否属于 mineral 聚合来源，需要逐曲显示 source
    ///   - `thumbnails`: 当前图片协议是否支持行内 Kitty 封面
    fn new(width: u16, aggregate: bool, thumbnails: bool) -> Self {
        let full_threshold = if aggregate { 68 } else { 56 };
        Self {
            full: width >= full_threshold,
            aggregate,
            thumbnails,
        }
    }

    /// 返回 title 列下标，供 marquee 与 loading 行使用同一列位置。
    fn title_index(self) -> usize {
        1 + usize::from(self.thumbnails)
    }

    /// 表头单元格(与 [`Self::widths`] / [`build_row`] 的列集严格一致)。
    fn header_cells(self) -> Vec<Cell<'static>> {
        let mut cells = vec![Cell::from("")];
        if self.thumbnails {
            cells.push(Cell::from(""));
        }
        cells.push(Cell::from("title"));
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
        let mut widths = vec![Constraint::Length(1)];
        if self.thumbnails {
            widths.push(Constraint::Length(THUMBNAIL_COLUMNS));
        }
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

/// 用本次视口协调列表状态，并声明可见行的图片需求。
impl crate::components::lifecycle::Prepare for TrackList {
    type Input<'a> = TrackInput<'a>;

    /// 更新本列表的显示状态；图片只声明需求，交由组合根协调。
    fn prepare(&mut self, area: Rect, input: TrackInput<'_>, cx: &mut PrepareCx<'_>) {
        let frame = cx.frame;
        let theme = frame.theme;
        let advance = cx.advance;
        self.stable = matches!(cx.motion, ScrollMotion::Advancing { .. });
        let total = self.rows(input).len();
        let motion = cx.motion;
        let ticks = frame.cursor_ticks();
        let viewport = usize::from(area.height.saturating_sub(3));
        self.scroll.prepare(total, viewport, motion, ticks, advance);
        let aggregate = input
            .playlist
            .is_some_and(|p| p.data.source() == SourceKind::MINERAL);
        let layout = TrackLayout::new(
            area.width,
            aggregate,
            cx.images.ready().supports_thumbnails(),
        );
        let inner = Block::new().borders(Borders::ALL).inner(area);
        let columns =
            resolve_column_rects(inner, &layout.widths(), display_width(HIGHLIGHT_SYMBOL));
        let title_w = columns.get(layout.title_index()).map_or(0, |r| r.width);
        let selected = self.scroll.sel();
        let offset = self.scroll.offset(total, viewport);
        let visible = offset..offset.saturating_add(viewport).min(total);
        let (title, covers, playing) = {
            let tracks = self.rows(input);
            let title = visible
                .contains(&selected)
                .then(|| tracks.get(selected))
                .flatten()
                .map(|entry| {
                    (
                        &entry.data.song.id,
                        crate::components::layout::shared::marquee::song_title_width(
                            &entry.data.song,
                        ),
                    )
                })
                .map(|(id, width)| (id.qualified(), width));
            let covers = visible
                .clone()
                .map(|index| {
                    tracks
                        .get(index)
                        .and_then(|entry| entry.data.song.cover_url.clone())
                })
                .collect::<Vec<_>>();
            let playing = tracks
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    input
                        .playing
                        .filter(|song| **song == entry.data.song.id)
                        .map(|_| index)
                })
                .collect::<Vec<_>>();
            (title, covers, playing)
        };
        if let Some((identity, width)) = title {
            self.title.prepare(&identity, width, title_w, frame.now);
        }
        if layout.thumbnails
            && let Some(column) = columns.get(1)
        {
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
            playing.into_iter(),
            motion,
            ticks,
            frame.config.minimap(),
            advance,
        );
        if matches!(motion, ScrollMotion::Advancing { .. }) {
            let filtered = (!self.search.query().is_empty()).then(|| {
                use crate::components::layout::shared::scroll_table::{
                    PreparedMinimap, PreparedTable,
                };
                let tracks = self.rows(input);
                let rows = visible
                    .clone()
                    .filter_map(|index| tracks.get(index))
                    .map(|entry| entry.data.index)
                    .collect();
                let (table, columns, layout) = table(
                    area,
                    &self.view(input, frame, cx.images.ready()),
                    theme,
                    visible.clone(),
                );
                let images =
                    columns
                        .get(1)
                        .filter(|_| layout.thumbnails)
                        .map_or_else(Vec::new, |column| {
                            crate::components::layout::shared::thumbnails::snapshot_thumbnails(
                                cx.images.ready(),
                                *column,
                                covers.iter().map(Option::as_ref),
                            )
                        });
                let entries = tracks
                    .iter()
                    .enumerate()
                    .filter_map(|(index, entry)| {
                        let playing = input
                            .playing
                            .is_some_and(|song| *song == entry.data.song.id);
                        (entry.loved || playing).then_some(MinimapEntry {
                            index,
                            loved: entry.loved,
                            playing,
                        })
                    })
                    .collect();
                let content = PreparedTable {
                    table,
                    area,
                    border: None,
                    images,
                    selected: (total > 0).then(|| {
                        crate::runtime::scroll::viewport::pin_cursor(selected, offset, viewport)
                            .saturating_sub(offset)
                    }),
                    minimap: Some(PreparedMinimap {
                        area: super::super::preparation::minimap_track(area),
                        total,
                        cursor: MinimapCursor::new(&self.scroll, total, frame.config.minimap()),
                        entries,
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

/// 渲染 Library 视图到给定 [`Buffer`](正常渲染与离屏过渡合成共用此入口)。
impl TrackView<'_> {
    /// 根据已准备的输入绘制，重复调用不推进本组件。
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
        let tracks = view.rows();
        // 视口行数 = 面板高 - 上下边框 - 表头;offset 跨帧持久(nvim 手感),滚动经缓动平移。
        // view sweep 离屏帧与全屏 morph 瞬态布局均冻结视口，不用临时高度改写滚动目标。
        let viewport = usize::from(area.height.saturating_sub(3));
        let offset = view.list.scroll.offset(tracks.len(), viewport);
        let window = offset..offset.saturating_add(viewport).min(tracks.len());
        let (table, columns, layout) = table(area, view, theme, window);
        let visible = render_scroll_table(
            buf,
            area,
            |_| table,
            &view.list.scroll,
            tracks.len(),
            viewport,
        );
        if layout.thumbnails
            && let Some(column) = columns.get(1)
        {
            render_table_thumbnails(
                buf,
                view.images,
                *column,
                visible.clone().map(|index| {
                    tracks
                        .get(index)
                        .and_then(|entry| entry.data.song.cover_url.as_ref())
                }),
            );
        }
        let cursor =
            MinimapCursor::new(&view.list.scroll, tracks.len(), view.frame.config.minimap());
        let entries = tracks
            .iter()
            .enumerate()
            .map(|(index, entry)| MinimapEntry {
                index,
                loved: entry.loved,
                // 播放态只有歌曲身份；歌单中同曲的各个位置都标出，不借用队列下标冒充歌单位置。
                playing: view
                    .input
                    .playing
                    .is_some_and(|song| *song == entry.data.song.id),
            });
        // 右边框从表头行一直到底部计数行，覆盖整份列表。
        render_minimap(
            buf,
            Rect::new(
                area.right().saturating_sub(1),
                area.y.saturating_add(1),
                area.width.min(1),
                area.height.saturating_sub(2),
            ),
            tracks.len(),
            cursor,
            entries,
            theme,
        );
        list_expansion::finish_list(buf, active, theme, surface, visible.clone());
    }
}

/// 构造一个可见窗口的表格输入，供本次绘制与搜索展开旧端共用。
fn table(
    area: Rect,
    view: &TrackView<'_>,
    theme: &Theme,
    visible: std::ops::Range<usize>,
) -> (Table<'static>, Vec<Rect>, TrackLayout) {
    let title = view.input.playlist.map_or_else(
        || "tracks".to_owned(),
        |p| format!("tracks / {}", p.data.name),
    );

    let tracks = view.rows();
    // 未知时长的曲目不计入合计(只反映已知部分)。
    let total_min = tracks.total_duration_ms() / 60_000;
    let playlist_tracks = view.input.tracks;
    let has_more = playlist_tracks.is_some_and(|tracks| !tracks.complete);
    let duration_status = if playlist_tracks.is_some_and(|tracks| tracks.complete) {
        "total"
    } else {
        "more"
    };
    let pos = position_label(view.list.scroll.sel(), tracks.len(), has_more);

    // 左上角 source 徽标:标出当前歌单挂靠的来源(聚合面挂靠 mineral,单源面挂靠其真实
    // 来源),与 sidebar playlists 面的 source 列同色,离开 sidebar(全屏)时仍能辨源。
    let mut title_spans = Vec::new();
    if let Some(p) = view.input.playlist {
        let src = p.data.source();
        title_spans.push(Span::styled(
            format!(" {}", src.label()),
            Style::new().fg(crate::render::theme::resolve_source_color(
                theme,
                view.frame.config.source_colors(),
                src,
            )),
        ));
    }
    title_spans.push(Span::styled(
        format!(" {title} "),
        Style::new().fg(theme.subtext),
    ));
    title_spans.extend(search_badge(&view.list.search, None, theme));

    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(title_spans))
        .title_bottom(Line::from(pos).style(Style::new().fg(theme.overlay)))
        .title_bottom(
            Line::from(format!("{total_min}m {duration_status}"))
                .right_aligned()
                .style(Style::new().fg(theme.overlay)),
        );

    // 按面板宽度 × 是否聚合面选布局:窄屏放不下 artist/album 时退到「歌本身」
    // (♥ title len);聚合面(source = mineral 的跨源歌单)宽档额外带 per-song source
    // 表示。跨源的只有 mineral 源歌单,故看歌单 source 而非遍历曲目。
    let aggregate = view
        .input
        .playlist
        .is_some_and(|p| p.data.source() == SourceKind::MINERAL);
    let layout = TrackLayout::new(area.width, aggregate, view.images.supports_thumbnails());
    let placeholder = slot_placeholder(view, theme, layout);

    let header = Row::new(layout.header_cells())
        .style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));

    let widths = layout.widths();
    // 表格选中行的 fade 实际会被 row_highlight_style 整行 fg 盖掉(刻意保留整行
    // accent,见 MarqueeCtx::fade_to 注);fade_to 仍按其底色给,不误导插值方向。
    let marquee_ctx = MarqueeCtx::new(
        &view.list.title,
        view.frame.config.animation(),
        view.frame.now,
        theme,
        /*fade_to*/ theme.surface0,
    );
    let columns = resolve_column_rects(block.inner(area), &widths, display_width(HIGHLIGHT_SYMBOL));
    let title_w = columns.get(layout.title_index()).map_or(0, |r| r.width);
    let sel = view.list.scroll.sel();
    let build_table = |visible: std::ops::Range<usize>| {
        let rows: Vec<Row<'_>> = if let Some(row) = placeholder {
            vec![row]
        } else {
            visible
                .filter_map(|i| tracks.get(i).map(|entry| (i, entry)))
                .map(|(i, sv)| {
                    let marquee = row_marquee(i == sel, &marquee_ctx, title_w);
                    build_row(sv, view, theme, layout, marquee)
                })
                .collect()
        };

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

    (build_table(visible), columns, layout)
}

/// 把一首歌组装成 library 表格的一行(loved 标记 / 在播行装饰 / 高亮搜索词)。
/// `layout` 决定列集:窄档省去 artist/album。
fn build_row(
    entry: &PlaylistEntryView,
    view: &TrackView<'_>,
    theme: &Theme,
    layout: TrackLayout,
    marquee: Option<RowMarquee<'_>>,
) -> Row<'static> {
    let song = &entry.data.song;
    let is_current = view
        .input
        .playing
        .is_some_and(|playing| *playing == song.id);
    let (title_fg, artist_fg, album_fg) = if is_current {
        (theme.accent, theme.accent, theme.accent)
    } else {
        (theme.text, theme.subtext, theme.overlay)
    };
    // 像 vim signcolumn 一样的 gutter:loved 显 ♥,否则空。永远占一格,
    // 不抖动后续列。
    let love_cell = if entry.loved {
        Cell::from(Span::styled("♥", Style::new().fg(theme.red)))
    } else {
        Cell::from("")
    };

    let name_hits = view.list.search.match_for(&song.name).map(|m| m.hits);
    let mut title_spans = highlight_indices(
        &song.name,
        name_hits.as_deref().unwrap_or(&[]),
        Style::new().fg(title_fg),
        theme,
    );
    // alias(译名 / 副标题)是歌名的暗色括注后缀;命中字符与主字段同款 search_hit
    // 高亮。hits 是相对 alias 文本的 char 下标。
    if let Some(alias) = song.alias.as_deref() {
        let alias_hits = view.list.search.match_for(alias).map(|m| m.hits);
        title_spans.extend(alias_suffix(
            alias,
            alias_hits.as_deref().unwrap_or(&[]),
            theme,
        ));
    }
    let title_cell = match marquee {
        Some(m) => Cell::from(m.ctx.line(title_spans, &song.id.qualified(), m.title_w)),
        None => Cell::from(Line::from(title_spans)),
    };

    let len = format_ms_opt(song.duration_ms);

    let mut cells = vec![love_cell];
    if layout.thumbnails {
        cells.push(Cell::from(""));
    }
    cells.push(title_cell);
    if layout.full {
        let artist = song
            .artists
            .first()
            .map(|a| a.name.clone())
            .unwrap_or_default();
        let album = song
            .album
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_default();
        let artist_hits = view.list.search.match_for(&artist).map(|m| m.hits);
        let album_hits = view.list.search.match_for(&album).map(|m| m.hits);
        cells.push(Cell::from(Line::from(highlight_indices(
            &artist,
            artist_hits.as_deref().unwrap_or(&[]),
            Style::new().fg(artist_fg),
            theme,
        ))));
        cells.push(Cell::from(Line::from(highlight_indices(
            &album,
            album_hits.as_deref().unwrap_or(&[]),
            Style::new().fg(album_fg),
            theme,
        ))));
        if layout.aggregate {
            let src = song.source();
            cells.push(Cell::from(Span::styled(
                src.label(),
                Style::new().fg(crate::render::theme::resolve_source_color(
                    theme,
                    view.frame.config.source_colors(),
                    src,
                )),
            )));
        }
    }
    cells.push(Cell::from(len));
    let mut style = if song.unavailable {
        theme.unavailable_row()
    } else {
        Style::new()
    };
    if is_current {
        style = style.fg(theme.accent).add_modifier(Modifier::UNDERLINED);
    }
    let row = Row::new(cells);
    if style == Style::new() {
        row
    } else {
        row.style(style)
    }
}

/// 拼 ` n / loaded ` 的 footer 标签；还有未加载曲目时在数量后加 `+`。
fn position_label(sel: usize, total: usize, has_more: bool) -> String {
    let position = sel.saturating_add(1).min(total);
    let more = if has_more { "+" } else { "" };
    format!(" {position} / {total}{more} ")
}

/// 已打开歌单尚未拿到 tracks 时返回 loading 行;tracks 已到但搜索零命中时返回
/// 「无匹配」行;正常情况返回 `None`(走 tracks 渲染)。占位文本按 `layout` 落在 title 列。
fn slot_placeholder<'a>(
    view: &TrackView<'_>,
    theme: &Theme,
    layout: TrackLayout,
) -> Option<Row<'a>> {
    let placeholder_row = |text: &'static str| {
        let mut cells = vec![Cell::from(""); layout.title_index()];
        cells.push(Cell::from(Span::styled(
            text,
            Style::new().fg(theme.overlay),
        )));
        Row::new(cells)
    };
    if view.input.tracks.is_none() {
        return view.input.playlist.map(|_| placeholder_row("loading…"));
    }
    if !view.list.search.query().is_empty() && view.rows().is_empty() {
        return Some(placeholder_row("无匹配"));
    }
    None
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::runtime::state::AppState;

    /// 用本视图渲染入口画一帧(`60×12` ⇒ body 视口 = 12 - 边框 2 - 表头 1 = 9 行)。
    fn draw_lib(t: &mut Terminal<TestBackend>, state: &mut AppState) -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        t.draw(|f| {
            let area = f.area();
            crate::view::browse::prepare(area, state, &theme, true);
            crate::view::browse::tracks(state, &theme).paint(area, f.buffer_mut());
        })?;
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
            draw_lib(&mut t, &mut app.state)?;
        }
        let before = app.state.ui.browse.tracks.scroll.scroll_target();
        assert!(before > 0, "前置:视口已滚到深处");

        // 进入 morph(fullscreen 离开 at_min),面板高度逐帧收缩地渲染。
        let mut fs = Toggle::new(8);
        fs.set(true);
        fs.tick();
        app.state.ui.browse.fullscreen = fs;
        for h in (2..20_u16).rev() {
            let mut small = Terminal::new(TestBackend::new(60, h))?;
            draw_lib(&mut small, &mut app.state)?;
        }
        assert_eq!(
            app.state.ui.browse.tracks.scroll.scroll_target(),
            before,
            "morph 期间滚动目标不得被瞬态 viewport 改写"
        );

        // 回浏览态:渲染收敛后仍在原 offset(无重定目标 = 无平移)。
        app.state.ui.browse.fullscreen = Toggle::new(8);
        for _ in 0..10 {
            draw_lib(&mut t, &mut app.state)?;
        }
        assert_eq!(
            app.state.ui.browse.tracks.scroll.scroll_target(),
            before,
            "回浏览态视口首行应与进全屏前一致"
        );
        Ok(())
    }

    /// 过滤重排保持条目的原始 relation index。
    #[test]
    fn library_filtered_reorder_preserves_relation_indexes() -> color_eyre::Result<()> {
        use crate::runtime::view_model::PlaylistEntryView;
        use mineral_model::{CollectionIndex, PlaylistEntry, PlaylistId, SourceKind};
        use mineral_test::{song, with_name};

        let mut state = crate::test_support::state_with_tracks()?;
        let entry = |index, id, name| PlaylistEntryView {
            data: PlaylistEntry::builder()
                .index(CollectionIndex::new(index))
                .song(with_name(song(id), name))
                .build(),
            loved: false,
            plays: None,
        };
        state.models.library.tracks.insert(
            PlaylistId::new(SourceKind::NETEASE, "p1"),
            crate::runtime::state::PlaylistTracks {
                entries: vec![entry(9, "spread", "A distant B"), entry(2, "exact", "AB")],
                complete: true,
                next_offset: None,
            },
        );
        state.ui.browse.tracks.search.set_query("ab");
        let indexes = state
            .filtered_tracks()
            .iter()
            .map(|item| item.data.index.get())
            .collect::<Vec<_>>();
        assert_eq!(indexes, vec![2, 9], "exact match 应排在 fuzzy match 前");
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
            .models
            .library
            .tracks
            .get_mut(&PlaylistId::new(SourceKind::NETEASE, "p1"))
            .and_then(|views| views.get_mut(2))
        {
            v.data.song = mineral_test::aliased_song();
        }
        state.ui.browse.tracks.search.set_query("Mayoiuta");
        let filtered = state.filtered_tracks();
        assert!(
            filtered
                .iter()
                .any(|entry| entry.data.song.alias.as_deref() == Some("Mayoiuta")),
            "搜别名应命中该曲"
        );
        assert!(
            filtered
                .iter()
                .all(|entry| entry.data.song.alias.as_deref() == Some("Mayoiuta")),
            "只有别名命中的曲应留下(歌名/艺人/专辑都不含该词)"
        );
        Ok(())
    }
}
