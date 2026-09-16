//! Library 视图渲染:展示当前选中歌单内的曲目。

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Row, Table};

use mineral_model::SourceKind;

use super::badge::search_badge;
use crate::components::layout::shared::highlight::{alias_suffix, highlight_indices};
use crate::components::layout::shared::list_minimap::{
    MinimapCursor, MinimapEntry, render_minimap,
};
use crate::components::layout::shared::marquee::{
    MarqueeCtx, RowMarquee, resolve_column_rects, row_marquee,
};
use crate::components::layout::shared::scroll_table::render_scroll_table;
use crate::components::layout::shared::text::display_width;
use crate::components::layout::shared::thumbnails::{
    THUMBNAIL_COLUMNS, render_table_thumbnails, thumbnail_phase,
};
use crate::render::theme::Theme;
use crate::runtime::format::format_ms_opt;
use crate::runtime::marquee::Slot;
use crate::runtime::scroll::list::ScrollMotion;
use crate::runtime::state::AppState;
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

/// 渲染 Library 视图到给定 [`Buffer`](正常渲染与离屏过渡合成共用此入口)。
pub fn render_to(buf: &mut Buffer, area: Rect, state: &AppState, theme: &Theme) {
    let title = state.selected_playlist().map_or_else(
        || "tracks".to_owned(),
        |p| format!("tracks / {}", p.data.name),
    );

    let tracks = state.filtered_tracks();
    // 未知时长的曲目不计入合计(只反映已知部分)。
    let total_min = tracks.total_duration_ms() / 60_000;
    let playlist_tracks = state
        .selected_playlist()
        .and_then(|p| state.library.tracks.get(&p.data.id));
    let has_more = playlist_tracks.is_some_and(|tracks| !tracks.complete);
    let duration_status = if playlist_tracks.is_some_and(|tracks| tracks.complete) {
        "total"
    } else {
        "more"
    };
    let pos = position_label(state.browse.nav.track.sel(), tracks.len(), has_more);

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
            Line::from(format!("{total_min}m {duration_status}"))
                .right_aligned()
                .style(Style::new().fg(theme.overlay)),
        );

    // 按面板宽度 × 是否聚合面选布局:窄屏放不下 artist/album 时退到「歌本身」
    // (♥ title len);聚合面(source = mineral 的跨源歌单)宽档额外带 per-song source
    // 表示。跨源的只有 mineral 源歌单,故看歌单 source 而非遍历曲目。
    let aggregate = state
        .selected_playlist()
        .is_some_and(|p| p.data.source() == SourceKind::MINERAL);
    let layout = TrackLayout::new(area.width, aggregate, state.images.supports_thumbnails());
    let placeholder = slot_placeholder(state, theme, layout);

    let header = Row::new(layout.header_cells())
        .style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));

    let widths = layout.widths();
    // 表格选中行的 fade 实际会被 row_highlight_style 整行 fg 盖掉(刻意保留整行
    // accent,见 MarqueeCtx::fade_to 注);fade_to 仍按其底色给,不误导插值方向。
    let marquee_ctx = MarqueeCtx::new(state, theme, /*fade_to*/ theme.surface0);
    let columns = resolve_column_rects(block.inner(area), &widths, display_width(HIGHLIGHT_SYMBOL));
    let title_w = columns.get(layout.title_index()).map_or(0, |r| r.width);
    let sel = state.browse.nav.track.sel();
    let build_table = |visible: std::ops::Range<usize>| {
        let rows: Vec<Row<'_>> = if let Some(row) = placeholder {
            vec![row]
        } else {
            visible
                .filter_map(|i| tracks.get(i).map(|entry| (i, entry)))
                .map(|(i, sv)| {
                    let marquee =
                        row_marquee(i == sel, &marquee_ctx, Slot::BrowseSelected, title_w);
                    build_row(sv, state, theme, layout, marquee)
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

    // 视口行数 = 面板高 - 上下边框 - 表头;offset 跨帧持久(nvim 手感),滚动经缓动平移。
    // view sweep 离屏帧与全屏 morph 瞬态布局均冻结视口，不用临时高度改写滚动目标。
    let viewport = usize::from(area.height.saturating_sub(3));
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
    let visible = render_scroll_table(
        buf,
        area,
        build_table,
        &state.browse.nav.track,
        tracks.len(),
        viewport,
        motion,
    );
    if layout.thumbnails
        && let Some(column) = columns.get(1)
    {
        render_table_thumbnails(
            buf,
            &state.images,
            *column,
            visible.map(|index| {
                tracks
                    .get(index)
                    .and_then(|entry| entry.data.song.cover_url.as_ref())
            }),
            thumbnail_phase(state, motion, state.browse.nav.last_sel_change),
        );
    }
    let cursor = MinimapCursor::new(
        &state.browse.nav.track,
        tracks.len(),
        motion,
        state.minimap_cursor_ticks(),
        state.cfg.tui().minimap(),
    );
    let entries = tracks
        .iter()
        .enumerate()
        .map(|(index, entry)| MinimapEntry {
            index,
            loved: entry.loved,
            // 播放态只有歌曲身份；歌单中同曲的各个位置都标出，不借用队列下标冒充歌单位置。
            playing: state
                .playback
                .track
                .as_ref()
                .is_some_and(|song| song.id == entry.data.song.id),
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
}

/// 把一首歌组装成 library 表格的一行(loved 标记 / 在播行装饰 / 高亮搜索词)。
/// `layout` 决定列集:窄档省去 artist/album。
fn build_row<'a>(
    entry: &'a PlaylistEntryView,
    state: &AppState,
    theme: &Theme,
    layout: TrackLayout,
    marquee: Option<RowMarquee<'_>>,
) -> Row<'a> {
    let song = &entry.data.song;
    let is_current = state
        .playback
        .track
        .as_ref()
        .is_some_and(|playing| playing.id == song.id);
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

    let name_hits = state.browse.search.match_for(&song.name).map(|m| m.hits);
    let mut title_spans = highlight_indices(
        &song.name,
        name_hits.as_deref().unwrap_or(&[]),
        Style::new().fg(title_fg),
        theme,
    );
    // alias(译名 / 副标题)是歌名的暗色括注后缀;命中字符与主字段同款 search_hit
    // 高亮。hits 是相对 alias 文本的 char 下标。
    if let Some(alias) = song.alias.as_deref() {
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
                .line(title_spans, m.slot, &song.id.qualified(), m.title_w),
        ),
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
        let artist_hits = state.browse.search.match_for(&artist).map(|m| m.hits);
        let album_hits = state.browse.search.match_for(&album).map(|m| m.hits);
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
                    state.cfg.sources(),
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

/// 选中歌单尚未拿到 tracks 时返回 loading 行;tracks 已到但搜索零命中时返回
/// 「无匹配」行;正常情况返回 `None`(走 tracks 渲染)。占位文本按 `layout` 落在 title 列。
fn slot_placeholder<'a>(state: &AppState, theme: &Theme, layout: TrackLayout) -> Option<Row<'a>> {
    let placeholder_row = |text: &'static str| {
        let mut cells = vec![Cell::from(""); layout.title_index()];
        cells.push(Cell::from(Span::styled(
            text,
            Style::new().fg(theme.overlay),
        )));
        Row::new(cells)
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
    use crate::runtime::state::{AppState, View};

    /// 用本视图渲染入口画一帧(`60×12` ⇒ body 视口 = 12 - 边框 2 - 表头 1 = 9 行)。
    fn draw_lib(t: &mut Terminal<TestBackend>, state: &AppState) -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, state, &theme);
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

    /// 两侧页脚随完整性更新；首批尚未到达时也不能把时长标为 total。
    #[test]
    fn tracks_footer_marks_more_pages() -> color_eyre::Result<()> {
        let mut app = crate::test_support::app_with_long_library(500, 0)?;
        let id = app
            .state
            .selected_playlist()
            .ok_or_else(|| color_eyre::eyre::eyre!("playlist"))?
            .data
            .id
            .clone();
        let tracks = app
            .state
            .library
            .tracks
            .get_mut(&id)
            .ok_or_else(|| color_eyre::eyre::eyre!("tracks"))?;
        tracks.complete = false;
        tracks.next_offset = Some(500);
        let mut terminal = Terminal::new(TestBackend::new(60, 12))?;
        draw_lib(&mut terminal, &app.state)?;
        assert!(row(&terminal, 11).contains("1 / 500+"));
        assert!(row(&terminal, 11).contains("m more"));
        assert!(!row(&terminal, 11).contains("m total"));
        let tracks = app
            .state
            .library
            .tracks
            .get_mut(&id)
            .ok_or_else(|| color_eyre::eyre::eyre!("tracks"))?;
        tracks.complete = true;
        tracks.next_offset = None;
        draw_lib(&mut terminal, &app.state)?;
        assert!(row(&terminal, 11).contains("1 / 500 "));
        assert!(!row(&terminal, 11).contains("500+"));
        assert!(row(&terminal, 11).contains("m total"));
        assert!(!row(&terminal, 11).contains("m more"));
        app.state.library.tracks.remove(&id);
        draw_lib(&mut terminal, &app.state)?;
        assert!(row(&terminal, 11).contains("0m more"));
        assert_eq!(super::position_label(0, 0, false), " 0 / 0 ");
        Ok(())
    }

    /// 缓动大跳逐帧保持封面与文本同一视口，sweep 与形变冻结视口但保留封面。
    #[test]
    fn thumbnails_follow_scroll_and_remain_visible_during_transitions() -> color_eyre::Result<()> {
        use std::sync::Arc;

        use mineral_model::{MediaUrl, PlaylistId, SourceKind};
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        use crate::image::{ImageEngine, ImageRenderPhase};
        use crate::runtime::scroll::list::ScrollMotion;

        let theme = crate::test_support::default_theme()?;
        let mut app = crate::test_support::app_with_long_library(30, 0)?;
        let state = &mut app.state;
        state.browse.view.retempo(1);
        state.browse.view.tick();
        state.images = ImageEngine::disabled_kitty(Arc::clone(&state.cfg));
        let entries = state
            .library
            .tracks
            .get_mut(&PlaylistId::new(SourceKind::NETEASE, "p1"))
            .ok_or_else(|| color_eyre::eyre::eyre!("缺少测试曲目"))?;
        let mut previews = Vec::new();
        for (index, entry) in entries.iter_mut().enumerate() {
            if index % 3 == 1 {
                entry.data.song.cover_url = None;
                previews.push(None);
                continue;
            }
            let url = MediaUrl::remote(&format!("https://example.com/track-{index}.png"))?;
            entry.data.song.cover_url = Some(url.clone());
            state.images.insert_test_thumbnail(&url)?;
            let mut probe = Buffer::empty(Rect::new(0, 0, super::THUMBNAIL_COLUMNS, 1));
            state.images.render_thumbnail(
                Some(&url),
                probe.area,
                &mut probe,
                ImageRenderPhase::Stable,
            );
            previews.push(
                probe
                    .cell((0, 0))
                    .map(|cell| (cell.symbol().to_owned(), cell.fg, cell.underline_color)),
            );
        }
        let area = Rect::new(4, 3, 60, 8);
        let viewport = usize::from(area.height - 3);
        let motion = ScrollMotion::Advancing {
            scrolloff: state.scrolloff(),
            glide_ticks: state.list_glide_ticks(),
        };
        let mut thumbnail_x = None;
        for selected in [0, 20, 1] {
            state.browse.nav.track.set_sel(selected);
            for _ in 0..4 {
                let reference = state.browse.nav.track.clone();
                let offset = reference.offset(previews.len(), viewport, motion);
                let mut buf = Buffer::empty(area);
                super::render_to(&mut buf, area, state, &theme);
                assert_eq!(
                    state
                        .browse
                        .nav
                        .track
                        .offset(previews.len(), viewport, ScrollMotion::Frozen),
                    offset,
                    "每帧只推进一次视口"
                );
                let title_x = (area.left()..area.right())
                    .find(|&x| buf.cell((x, area.y + 1)).is_some_and(|c| c.symbol() == "t"))
                    .ok_or_else(|| color_eyre::eyre::eyre!("缺少 title 表头"))?;
                thumbnail_x = Some(title_x - super::THUMBNAIL_COLUMNS - 1);
                for row in 0..viewport {
                    let index = offset + row;
                    let y = area.y + 2 + u16::try_from(row)?;
                    let title = (title_x..area.right())
                        .filter_map(|x| buf.cell((x, y)))
                        .map(ratatui::buffer::Cell::symbol)
                        .collect::<String>();
                    assert!(
                        title.starts_with(&format!("Track {index:02}")),
                        "文本须来自本帧视口"
                    );
                    let cell = buf
                        .cell((title_x - super::THUMBNAIL_COLUMNS - 1, y))
                        .ok_or_else(|| color_eyre::eyre::eyre!("缺少图片格"))?;
                    if let Some((symbol, foreground, underline)) =
                        previews.get(index).and_then(Option::as_ref)
                    {
                        assert_eq!(
                            (cell.symbol(), cell.fg, cell.underline_color),
                            (symbol.as_str(), *foreground, *underline),
                            "第 {index} 曲的 Kitty image id 与 placement 必须和 title 同行"
                        );
                    } else {
                        assert_eq!(cell.symbol(), " ", "没有封面的行应留空");
                    }
                    assert_eq!(
                        buf.cell((title_x - 1, y))
                            .map(ratatui::buffer::Cell::symbol),
                        Some(" ")
                    );
                }
            }
        }
        state.browse.view.retempo(8);
        state.browse.view.switch_to(View::Playlists);
        state.browse.view.tick();
        let offset = state
            .browse
            .nav
            .track
            .offset(previews.len(), viewport, ScrollMotion::Frozen);
        let mut offscreen = Buffer::empty(area);
        super::render_to(&mut offscreen, area, state, &theme);
        assert!(
            offscreen
                .content
                .iter()
                .all(|c| !c.symbol().contains('\x1b')),
            "sweep 离屏帧不得含图形控制序列"
        );
        let x = thumbnail_x.ok_or_else(|| color_eyre::eyre::eyre!("缺少图片列"))?;
        assert_eq!(
            state
                .browse
                .nav
                .track
                .offset(previews.len(), viewport, ScrollMotion::Frozen),
            offset
        );
        for row in 0..viewport {
            let cell = offscreen
                .cell((x, area.y + 2 + u16::try_from(row)?))
                .ok_or_else(|| color_eyre::eyre::eyre!("缺少离屏图片格"))?;
            if let Some((symbol, foreground, underline)) =
                previews.get(offset + row).and_then(Option::as_ref)
            {
                assert_eq!(
                    (cell.symbol(), cell.fg, cell.underline_color),
                    (symbol.as_str(), *foreground, *underline),
                    "sweep 冻结视口后仍保留对应封面"
                );
            } else {
                assert_eq!(cell.symbol(), " ");
            }
        }
        state.browse.view.retempo(1);
        state.browse.view.switch_to(View::Library);
        state.browse.view.tick();
        state.browse.fullscreen = crate::render::anim::Toggle::new(8);
        state.browse.fullscreen.set(true);
        state.browse.fullscreen.tick();
        let mut morph = Buffer::empty(area);
        super::render_to(&mut morph, area, state, &theme);
        assert!(
            morph.content.iter().all(|c| !c.symbol().contains('\x1b')),
            "fullscreen morph 不得输出图形控制序列"
        );
        for row in 0..viewport {
            let y = area.y + 2 + u16::try_from(row)?;
            for column in x..x + super::THUMBNAIL_COLUMNS {
                assert_eq!(
                    morph.cell((column, y)),
                    offscreen.cell((column, y)),
                    "形变与 sweep 复用同一封面格"
                );
            }
        }
        Ok(())
    }

    /// G 滚到底后向上走时,光标留在 scrolloff 安全区内不会移动视口;
    /// 只有越过安全边界才向上滚动。
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

    /// minimap 使用过滤后的顺序；歌单里当前歌曲的各个副本都可见，空命中不残留标记。
    #[test]
    fn minimap_follows_filtered_order_and_playing_song_identity() -> color_eyre::Result<()> {
        use crate::runtime::view_model::PlaylistEntryView;
        use mineral_model::{CollectionIndex, PlaylistEntry, PlaylistId, SourceKind};

        let mut state = crate::test_support::state_with_tracks()?;
        let playing = mineral_test::song("keep");
        state.playback.track = Some(playing.clone());
        let entries = [mineral_test::song("skip"), playing.clone(), playing]
            .into_iter()
            .zip([7, 17, 27])
            .map(|(song, index)| PlaylistEntryView {
                data: PlaylistEntry::builder()
                    .index(CollectionIndex::new(index))
                    .song(song)
                    .build(),
                loved: false,
                plays: None,
            })
            .collect();
        state.library.tracks.insert(
            PlaylistId::new(SourceKind::NETEASE, "p1"),
            crate::runtime::state::PlaylistTracks {
                entries,
                complete: true,
                next_offset: None,
            },
        );
        state.browse.search.set_query("keep");
        state.browse.nav.track.place(0, 0);
        let mut terminal = Terminal::new(TestBackend::new(60, 12))?;
        draw_lib(&mut terminal, &state)?;
        let buffer = terminal.backend().buffer();
        // 表头行也属于轨道：两个过滤命中（同一首歌的两份）落在首末行。
        assert_eq!(
            buffer.cell((59, 1)).map(ratatui::buffer::Cell::symbol),
            Some("◆")
        );
        assert_eq!(
            buffer.cell((59, 10)).map(ratatui::buffer::Cell::symbol),
            Some("◆")
        );
        assert_eq!(
            buffer.cell((59, 11)).map(ratatui::buffer::Cell::symbol),
            Some("╯")
        );
        state.browse.search.set_query("absent");
        draw_lib(&mut terminal, &state)?;
        assert!((1..11).all(|y| {
            terminal
                .backend()
                .buffer()
                .cell((59, y))
                .is_some_and(|cell| cell.symbol() == "⢸")
        }));
        Ok(())
    }

    /// 已选歌单 + 3 首曲目(CJK 歌名 / 收藏)。
    #[test]
    fn library_with_tracks_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(80, 12))?;
        let state = crate::test_support::state_with_tracks()?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &theme);
        })?;
        crate::test_support::assert_snap!("曲目列表:EndSerenading 前 3 曲(♥ 收藏)", t.backend());
        Ok(())
    }

    /// `/` filter 按 fuzzy score 重排曲目，保留条目原有的 CollectionIndex。
    #[test]
    fn library_filtered_reorder_preserves_relation_indexes_snapshot() -> color_eyre::Result<()> {
        use mineral_model::{CollectionIndex, PlaylistEntry, PlaylistId, SourceKind};
        use mineral_test::{song, with_name};

        use crate::runtime::view_model::PlaylistEntryView;

        let mut state = crate::test_support::state_with_tracks()?;
        let entry = |index, id, name| PlaylistEntryView {
            data: PlaylistEntry::builder()
                .index(CollectionIndex::new(index))
                .song(with_name(song(id), name))
                .build(),
            loved: false,
            plays: None,
        };
        state.library.tracks.insert(
            PlaylistId::new(SourceKind::NETEASE, "p1"),
            crate::runtime::state::PlaylistTracks {
                entries: vec![entry(9, "spread", "A distant B"), entry(2, "exact", "AB")],
                complete: true,
                next_offset: None,
            },
        );
        state.browse.nav.track.set_sel(0);
        state.browse.search.set_query("ab");

        let indexes = state
            .filtered_tracks()
            .iter()
            .map(|item| item.data.index.get())
            .collect::<Vec<_>>();
        assert_eq!(indexes, vec![2, 9], "exact match 应排在 fuzzy match 前");

        let mut terminal = Terminal::new(TestBackend::new(60, 12))?;
        draw_lib(&mut terminal, &state)?;
        crate::test_support::assert_snap!(
            "曲目列表:filter 按匹配分重排，歌名与选中行对齐",
            terminal.backend()
        );
        Ok(())
    }

    /// 左上角 source 徽标:单源歌单染该歌单真实 source 色,聚合面(mineral)染 mineral 色,
    /// 与 sidebar playlists 面 source 列同一套 [`resolve_source_color`]。
    #[test]
    fn library_title_badge_matches_source_color() -> color_eyre::Result<()> {
        use mineral_model::SourceKind;

        use crate::render::theme::resolve_source_color;

        let theme = crate::test_support::default_theme()?;
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
            v.data.song = mineral_test::aliased_song();
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
            v.data.song = mineral_test::aliased_song();
        }
        state.browse.search.set_query("Mayoiuta");
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

    /// 别名命中与主字段同款高亮:命中子串换 search_hit 色 + 字体效果,括号与未命中
    /// 别名字符保持 overlay 暗调。命中效果只在非选中行落地(选中行整行 fg 被
    /// row_highlight 的 accent 顶掉,见 render_to 注),故把选中放在第二行、别名命中行
    /// 留在第一行。只扫 body 行(y=0 的标题栏 query badge 也染 search_hit 色,须排除):
    /// body 里 search_hit 色的字符恰好拼成 "Mayo",其余别名字符仍是 overlay。
    #[test]
    fn alias_hits_use_primary_highlight() -> color_eyre::Result<()> {
        use mineral_model::{PlaylistId, SourceKind};

        use crate::test_support::{song, with_alias, with_name};

        let theme = crate::test_support::default_theme()?;
        let mut state = crate::test_support::state_with_tracks()?;
        // 前两首各带别名 Mayoiuta(歌名各异、都不含 "mayo"),搜 "mayo" 二者皆命中。
        if let Some(views) = state
            .library
            .tracks
            .get_mut(&PlaylistId::new(SourceKind::NETEASE, "p1"))
        {
            if let Some(v) = views.get_mut(0) {
                v.data.song = with_alias(with_name(song("s0"), "迷星叫"), "Mayoiuta");
            }
            if let Some(v) = views.get_mut(1) {
                v.data.song = with_alias(with_name(song("s1"), "叫喊迷星"), "Mayoiuta");
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

    /// 在播行与 queue 同款:文本列换 accent、整行下划线,时长格靠整行 fg 接住;
    /// 非在播行两样都不带,右边框轨道格也不吃行装饰。
    #[test]
    fn playing_row_takes_the_queue_decoration() -> color_eyre::Result<()> {
        use ratatui::style::Modifier;

        let theme = crate::test_support::default_theme()?;
        let mut state = crate::test_support::state_with_tracks()?;
        // 与渲染同源的可见顺序:首行设为在播(fixture 选中的是第二首,两行互不掩盖)。
        let (names, playing) = {
            let visible = state.filtered_tracks();
            (
                visible
                    .iter()
                    .map(|entry| entry.data.song.name.clone())
                    .collect::<Vec<_>>(),
                visible.first().map(|entry| entry.data.song.clone()),
            )
        };
        let playing = playing.ok_or_else(|| color_eyre::eyre::eyre!("缺少测试曲目"))?;
        let plain = names
            .get(2)
            .cloned()
            .ok_or_else(|| color_eyre::eyre::eyre!("缺少第三首测试曲目"))?;
        state.playback.track = Some(playing.clone());

        let mut t = Terminal::new(TestBackend::new(80, 12))?;
        draw_lib(&mut t, &state)?;

        let buf = t.backend().buffer();
        // y=2 起是曲目 body:按名字定位两行,标题列取表头 `title` 的起点。
        let body_row = |name: &str| (2..buf.area.height).find(|&y| row(&t, y).contains(name));
        let playing_y = body_row(&playing.name)
            .ok_or_else(|| color_eyre::eyre::eyre!("在播行未渲染: {}", playing.name))?;
        let plain_y =
            body_row(&plain).ok_or_else(|| color_eyre::eyre::eyre!("非在播行未渲染: {plain}"))?;
        let title_x = u16::try_from(
            row(&t, 1)
                .find("title")
                .ok_or_else(|| color_eyre::eyre::eyre!("缺少表头"))?,
        )?;
        let title = |y: u16| buf.cell((title_x, y));
        assert_eq!(
            title(playing_y).map(|c| c.fg),
            Some(theme.accent),
            "在播行标题用 accent(行文本 {:?})",
            row(&t, playing_y)
        );
        assert!(
            title(playing_y).is_some_and(|c| c.modifier.contains(Modifier::UNDERLINED)),
            "在播行整行加下划线"
        );
        assert_eq!(
            title(plain_y).map(|c| c.fg),
            Some(theme.text),
            "非在播行标题保持主文本色(行文本 {:?})",
            row(&t, plain_y)
        );
        assert!(
            title(plain_y).is_none_or(|c| !c.modifier.contains(Modifier::UNDERLINED)),
            "非在播行不带下划线"
        );
        // 时长格没有自己的前景,靠整行 fg 接住 accent。
        assert!(
            (0..buf.area.width)
                .filter_map(|x| buf.cell((x, playing_y)))
                .any(|c| c.symbol() == ":" && c.fg == theme.accent),
            "时长格也吃到整行 accent"
        );
        // 轨道画在面板边框列(表格区之外),不吃在播行的下划线。
        assert!(
            buf.cell((buf.area.width - 1, playing_y))
                .is_some_and(|c| !c.modifier.contains(Modifier::UNDERLINED)),
            "轨道格不吃行装饰"
        );
        Ok(())
    }

    /// 选中歌单但曲目未到(library.tracks 空)→ loading 态。
    #[test]
    fn library_loading_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(80, 12))?;
        let mut state = crate::test_support::state_with_playlists()?;
        state.browse.view.switch_to(View::Library);
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &theme);
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

    /// 聚合面 68 格起显示 source 列，普通面 56 格进宽档。
    #[test]
    fn aggregate_layout_threshold_leaves_room_for_source_column() {
        assert!(
            !TrackLayout::new(60, /*aggregate*/ true, /*thumbnails*/ false).full,
            "聚合面 60 格退窄档(插不起 source 列)"
        );
        assert!(
            TrackLayout::new(56, /*aggregate*/ false, /*thumbnails*/ false).full,
            "普通面 56 格进宽档"
        );
        assert!(
            TrackLayout::new(68, /*aggregate*/ true, /*thumbnails*/ false).full,
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

    /// CJK 曲目(Chinese Football)在 Full 档多列里的宽字符对齐(width=80)—— 含最长的
    /// 「不是人人都能穿十号球衣」,验证 title/artist/album 三列宽字符不串列。
    #[test]
    fn library_cjk_tracks_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(80, 12))?;
        let state = crate::test_support::state_with_cjk_tracks()?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &theme);
        })?;
        crate::test_support::assert_snap!(
            "曲目列表:CJK 曲目(Chinese Football)Full 档宽字符对齐",
            t.backend()
        );
        Ok(())
    }

    /// 窄面板(width=44 < 56)退到 Song 档:只剩 ♥ / title / len,artist/album 省去。
    #[test]
    fn library_narrow_song_snapshot() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(44, 12))?;
        let state = crate::test_support::state_with_tracks()?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &theme);
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
        let theme = crate::test_support::default_theme()?;
        let mut t = Terminal::new(TestBackend::new(80, 12))?;
        let state = crate::test_support::state_with_album()?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &theme);
        })?;
        crate::test_support::assert_snap!(
            "曲目列表:带 album 数据的 Full 档(title/artist/album 三列对齐)",
            t.backend()
        );
        Ok(())
    }
}
