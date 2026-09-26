//! Detail 面板栈顶帧的主体列表区:非 artist 帧曲目表、artist 帧热门曲/专辑双区(Tab + 切区
//! 离屏合成),及两张表(曲目/专辑)的渲染。数据未到画骨架,选中行高亮随面板焦点度渐变。

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, Widget};

use mineral_config::SweepStyle;
use mineral_model::{Album, AlbumTrack, PlaylistEntry, Song};

use crate::components::layout::shared::marquee::{MarqueeCtx, resolve_column_rects, row_marquee};
use crate::components::layout::shared::scroll_table::render_scroll_table;
use crate::components::layout::shared::text::display_width;
use crate::components::layout::shared::thumbnails::{
    THUMBNAIL_COLUMNS, render_table_thumbnails, thumbnail_phase,
};
use crate::render::theme::Theme;
use crate::runtime::marquee::Slot;
use crate::runtime::scroll::list::{ScrollList, ScrollMotion};
use crate::runtime::state::{
    AppState, ArtistSection, DetailData, DetailFrame, EntityRef, SearchFocus,
};

use super::geometry::split_artist_body;
use super::meta::{publish_year, with_commas};
use super::placeholder::{draw_empty, draw_loading, loading_glyph};
use super::sweep::{FULL, SweepLayer, copy_col, sweep_column};
use super::track_table::{self, TrackColumns, highlight_style};

/// 列表区渲染上下文:光标 + 视口态 + 本帧推进语义 + 面板焦点。穿过 body 渲染链时合并一束传,
/// 压参数个数。
#[derive(Clone, Copy)]
struct ListPaint<'a> {
    /// 该列表的光标 + 视口滚动态(取自栈顶帧)。
    list: &'a ScrollList,

    /// 本帧推进语义(稳态 Advancing / 离屏 Frozen)。
    motion: ScrollMotion,

    /// 面板焦点度(千分比):选中行高亮 subtext→accent 的插值参数,随焦点环滑动渐变,
    /// 与 results 列对称。
    focus_permille: u16,
}

/// 主体：artist 帧画双区，其余画曲目列表。`motion` 透传给底层列表(稳态推进 / 离屏冻结)。
pub(super) fn draw_body(
    buf: &mut Buffer,
    body: Rect,
    dframe: &DetailFrame,
    state: &AppState,
    theme: &Theme,
    motion: ScrollMotion,
) {
    match &dframe.entity {
        EntityRef::Artist(_) => draw_artist_body(buf, body, dframe, state, theme, motion),
        _ => draw_track_body(buf, body, dframe, state, theme, motion),
    }
}

/// 非 artist 帧主体：曲目列表（数据未到画骨架）。album / song 帧曲目来自 `Album.tracks`（同源
/// 专辑无需再列 album 列），歌单帧来自 `PlaylistEntries`（混源，多列出 album）。
fn draw_track_body(
    buf: &mut Buffer,
    body: Rect,
    dframe: &DetailFrame,
    state: &AppState,
    theme: &Theme,
    motion: ScrollMotion,
) {
    let paint = ListPaint {
        list: dframe.list(),
        motion,
        focus_permille: state.channel_search.focus_permille(
            *state.cfg.tui().animation().search_focus_transition(),
            SearchFocus::Detail,
        ),
    };
    match &dframe.data {
        Some(DetailData::Album(a)) => draw_track_list(
            buf,
            body,
            TrackList::Album(&a.tracks),
            paint,
            TrackColumns::new(/*artist*/ true, /*album*/ false),
            state,
            theme,
        ),
        Some(DetailData::PlaylistEntries(entries)) => draw_track_list(
            buf,
            body,
            TrackList::Playlist(entries),
            paint,
            TrackColumns::new(/*artist*/ true, /*album*/ true),
            state,
            theme,
        ),
        // 数据未到货 → 旋转 loading(非空态)。
        _ => draw_loading(buf, body, loading_glyph(state), theme),
    }
}

/// artist 帧主体：热门曲/专辑双区 Tab + 当前区列表。热门曲是该 artist 的歌，artist 列冗余，
/// 改列 album。
fn draw_artist_body(
    buf: &mut Buffer,
    body: Rect,
    dframe: &DetailFrame,
    state: &AppState,
    theme: &Theme,
    motion: ScrollMotion,
) {
    if body.height < 2 {
        return;
    }
    // 单区源(如 B站仅专辑):不画切换 tab,整块给该区列表(分区已收到那唯一可用区)。
    let single_section = dframe
        .artist_sections()
        .is_some_and(|sections| sections.kinds().len() < 2);
    if single_section {
        draw_artist_section(buf, body, dframe.section, dframe, state, theme, motion);
        return;
    }
    let (tabs, list) = split_artist_body(body);
    draw_artist_tabs(buf, tabs, dframe.section, theme);
    match dframe.section_eased() {
        // 切区滑动期：两区各渲染到离屏 Buffer，按进度横向合成（0=Top Songs、满值=Albums），
        // 风格尊重 `view_sweep` 配置——与左栏 playlists↔tracks 同款。Tab/头图不滑，只动列表区。
        // 两次离屏渲染共用同一份 ScrollList,必须 Frozen(只读、幂等),否则同帧双调 render_offset
        // 会双倍推进缓动。
        Some(eased) => {
            let mut hot_buf = Buffer::empty(list);
            let mut alb_buf = Buffer::empty(list);
            draw_artist_section(
                &mut hot_buf,
                list,
                ArtistSection::Hot,
                dframe,
                state,
                theme,
                ScrollMotion::Frozen,
            );
            draw_artist_section(
                &mut alb_buf,
                list,
                ArtistSection::Albums,
                dframe,
                state,
                theme,
                ScrollMotion::Frozen,
            );
            compose_sweep(
                buf,
                list,
                &hot_buf,
                &alb_buf,
                eased,
                *state.cfg.tui().animation().view_sweep(),
            );
        }
        None => draw_artist_section(buf, list, dframe.section, dframe, state, theme, motion),
    }
}

/// 画 artist 某一区的列表：Top Songs 走曲目表（artist 列冗余，出 album 列）、Albums 走专辑表；
/// 数据未到画骨架。切区滑动期对两区各调一次（离屏合成，传 `Frozen`）。
fn draw_artist_section(
    buf: &mut Buffer,
    list: Rect,
    section: ArtistSection,
    dframe: &DetailFrame,
    state: &AppState,
    theme: &Theme,
    motion: ScrollMotion,
) {
    let paint = ListPaint {
        list: dframe.list(),
        motion,
        focus_permille: state.channel_search.focus_permille(
            *state.cfg.tui().animation().search_focus_transition(),
            SearchFocus::Detail,
        ),
    };
    match (section, &dframe.data) {
        (
            ArtistSection::Hot,
            Some(DetailData::Artist {
                detail: Some(a), ..
            }),
        ) => draw_track_list(
            buf,
            list,
            TrackList::Songs(&a.songs),
            paint,
            TrackColumns::new(/*artist*/ false, /*album*/ true),
            state,
            theme,
        ),
        (
            ArtistSection::Albums,
            Some(DetailData::Artist {
                albums: Some(albs), ..
            }),
        ) => {
            if albs.items().is_empty() && albs.has_more() {
                if albs.is_loading() {
                    draw_loading(buf, list, loading_glyph(state), theme);
                } else {
                    draw_empty(buf, list, "more albums available", theme);
                }
            } else {
                draw_album_list(buf, list, albs.items(), paint, state, theme);
            }
        }
        // 该区数据未到货 → 旋转 loading。
        _ => draw_loading(buf, list, loading_glyph(state), theme),
    }
}

/// 两区离屏 buffer 按 `eased`（千分比，`0`=base、满值=over）横向合成到 `area`，尊重
/// [`SweepStyle`]（Push 整体平移 / Cover 新区从右覆盖）。与左栏 view-sweep 同范式。
fn compose_sweep(
    buf: &mut Buffer,
    area: Rect,
    base: &Buffer,
    over: &Buffer,
    eased: u16,
    style: SweepStyle,
) {
    let w = area.width;
    let advance = u16::try_from(u32::from(w) * u32::from(eased) / FULL)
        .unwrap_or(w)
        .min(w);
    for c in 0..w {
        // 双区切换恒「前进」（base=当前区 → over=目标区，over 从右来），故方向取 is_push=true；
        // 反向切换由 eased 回落表达，不需另一方向。与下钻 sweep 共用同一列映射。
        let (src, src_c) = match sweep_column(style, /*is_push*/ true, c, w, advance) {
            (SweepLayer::From, src_c) => (base, src_c),
            (SweepLayer::To, src_c) => (over, src_c),
        };
        copy_col(buf, area, src, c, src_c);
    }
}

/// artist 双区 Tab：当前区 accent 高亮，另一区暗调；右侧 `[ / ]` 切区提示。
fn draw_artist_tabs(buf: &mut Buffer, area: Rect, section: ArtistSection, theme: &Theme) {
    let on = Style::new().fg(theme.accent).add_modifier(Modifier::BOLD);
    let off = Style::new().fg(theme.subtext);
    let hot = if section == ArtistSection::Hot {
        on
    } else {
        off
    };
    let albums = if section == ArtistSection::Albums {
        on
    } else {
        off
    };
    let line = Line::from(vec![
        Span::styled("Top Songs", hot),
        Span::raw("  "),
        Span::styled("Albums", albums),
        Span::styled("   [ / ] section", Style::new().fg(theme.overlay)),
    ]);
    Widget::render(Paragraph::new(line), area, buf);
}

/// 曲目表（♥/title/[artist]/[album]/len，带表头）：对齐 browse library 表风格。
/// `cols` 选中间列、按面板宽度降级；`list` 选中行整行高亮 + nvim 视口滚动，
/// 已收藏显 `♥`。`motion` 定推进(稳态)/ 冻结(离屏)。
fn draw_track_list(
    buf: &mut Buffer,
    area: Rect,
    tracks: TrackList<'_>,
    paint: ListPaint<'_>,
    cols: TrackColumns,
    state: &AppState,
    theme: &Theme,
) {
    if tracks.is_empty() {
        // 已到货但 0 曲 → 静态空态(非 loading,数据已在手)。
        draw_empty(buf, area, "no tracks", theme);
        return;
    }
    let show_cover = state.images.supports_thumbnails();
    let cols = cols.with_thumbnails(show_cover).for_width(area.width);
    let widths = cols.widths();
    // 表格选中行的 fade 实际会被 row_highlight_style 整行 fg 盖掉(刻意保留整行
    // accent,见 MarqueeCtx::fade_to 注);fade_to 仍按其底色给,不误导插值方向。
    let marquee_ctx = MarqueeCtx::new(state, theme, /*fade_to*/ theme.surface0);
    let columns = resolve_column_rects(area, &widths, display_width(track_table::HIGHLIGHT_SYMBOL));
    let title_w = columns.get(cols.title_index()).map_or(0, |r| r.width);
    let sel = paint.list.sel();
    let build_table = |visible: std::ops::Range<usize>| {
        let rows = visible.filter_map(|view_index| {
            let song = tracks.song(view_index)?;
            let loved = state.is_liked(song);
            let marquee = row_marquee(
                view_index == sel,
                &marquee_ctx,
                Slot::SearchDetailSelected,
                title_w,
            );
            Some(track_table::track_row(song, loved, cols, theme, marquee))
        });
        Table::new(rows, widths)
            .header(track_table::header_row(cols, theme))
            .row_highlight_style(highlight_style(theme, paint.focus_permille))
            .highlight_symbol(track_table::HIGHLIGHT_SYMBOL)
    };
    // 视口行数 = 区高 - 表头(无 block 边框,area 已是内容区)。
    let viewport = usize::from(area.height.saturating_sub(1));
    let visible = render_scroll_table(
        buf,
        area,
        build_table,
        paint.list,
        tracks.len(),
        viewport,
        paint.motion,
    );
    if show_cover && let Some(column) = columns.get(1) {
        render_table_thumbnails(
            buf,
            &state.images,
            *column,
            visible.map(|index| tracks.song(index).and_then(|song| song.cover_url.as_ref())),
            thumbnail_phase(state, paint.motion, state.channel_search.last_sel_change),
        );
    }
}

/// 曲目表的数据源，按列表位置读取歌曲，供文本与封面共用同一视口。
#[derive(Clone, Copy)]
enum TrackList<'a> {
    /// 普通歌曲列表，如艺人热门曲目。
    Songs(&'a [Song]),

    /// 专辑曲目，按专辑提供的顺序显示。
    Album(&'a [AlbumTrack]),

    /// 歌单条目，按歌单提供的顺序显示。
    Playlist(&'a [PlaylistEntry]),
}

impl<'a> TrackList<'a> {
    /// 数据源是否为空。
    fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// 数据源长度。
    fn len(self) -> usize {
        match self {
            Self::Songs(songs) => songs.len(),
            Self::Album(tracks) => tracks.len(),
            Self::Playlist(entries) => entries.len(),
        }
    }

    /// 按列表位置读取歌曲，超出列表范围时返回 `None`。
    fn song(self, view_index: usize) -> Option<&'a Song> {
        match self {
            Self::Songs(songs) => songs.get(view_index),
            Self::Album(tracks) => tracks.get(view_index).map(|track| &track.song),
            Self::Playlist(entries) => entries.get(view_index).map(|entry| &entry.song),
        }
    }
}

/// 专辑表（name/tracks/year/label，带表头）：artist Albums 区，`list` 选中行整行高亮（下钻入口）。
fn draw_album_list(
    buf: &mut Buffer,
    area: Rect,
    albums: &[Album],
    paint: ListPaint<'_>,
    state: &AppState,
    theme: &Theme,
) {
    if albums.is_empty() {
        // 已到货但 0 专辑 → 静态空态。
        draw_empty(buf, area, "no albums", theme);
        return;
    }
    let meta = Style::new().fg(theme.overlay);
    let show_cover = state.images.supports_thumbnails();
    let mut header_cells = Vec::<Cell<'_>>::new();
    let mut widths = Vec::<Constraint>::new();
    if show_cover {
        header_cells.push(Cell::from(""));
        widths.push(Constraint::Length(THUMBNAIL_COLUMNS));
    }
    header_cells.extend([
        Cell::from("name"),
        Cell::from("tracks"),
        Cell::from("year"),
        Cell::from("label"),
    ]);
    widths.extend([
        Constraint::Fill(3),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Fill(2),
    ]);
    let header =
        Row::new(header_cells).style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));
    let columns = resolve_column_rects(area, &widths, display_width(track_table::HIGHLIGHT_SYMBOL));
    let build_table = |visible: std::ops::Range<usize>| {
        let rows = albums
            .iter()
            .skip(visible.start)
            .take(visible.len())
            .map(|a| {
                // 曲目数未知(搜索 / 投稿列表投影)画 `-`,别画 `0` 冒充空专辑;下钻 album_detail 回填真值。
                let tracks = a.track_count.map_or_else(|| "-".to_owned(), with_commas);
                let year =
                    publish_year(a.publish_time_ms).map_or_else(String::new, |y| y.to_string());
                let label = a.company.as_deref().unwrap_or_default().to_owned();
                let mut cells = Vec::<Cell<'_>>::new();
                if show_cover {
                    cells.push(Cell::from(""));
                }
                cells.extend([
                    Cell::from(Span::styled(a.name.clone(), Style::new().fg(theme.text))),
                    Cell::from(Span::styled(tracks, meta)),
                    Cell::from(Span::styled(year, meta)),
                    Cell::from(Span::styled(label, meta)),
                ]);
                Row::new(cells)
            });
        Table::new(rows, widths)
            .header(header)
            .row_highlight_style(highlight_style(theme, paint.focus_permille))
            .highlight_symbol(track_table::HIGHLIGHT_SYMBOL)
    };
    // 视口行数 = 区高 - 表头(无 block 边框,area 已是内容区)。
    let viewport = usize::from(area.height.saturating_sub(1));
    let visible = render_scroll_table(
        buf,
        area,
        build_table,
        paint.list,
        albums.len(),
        viewport,
        paint.motion,
    );
    if show_cover && let Some(column) = columns.first() {
        render_table_thumbnails(
            buf,
            &state.images,
            *column,
            visible.map(|index| albums.get(index).and_then(|album| album.cover_url.as_ref())),
            thumbnail_phase(state, paint.motion, state.channel_search.last_sel_change),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mineral_model::{Album, AlbumId, MediaUrl, SourceKind};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    use super::{ListPaint, TrackList, draw_album_list, draw_track_list};
    use crate::image::ImageEngine;
    use crate::runtime::scroll::list::{ScrollList, ScrollMotion};
    use crate::runtime::state::AppState;
    use crate::test_support::{song, with_name};

    /// 曲目和艺人专辑都在名称前显示封面，缺图保留行位置，离屏合成保留封面身份。
    #[test]
    fn detail_thumbnail_columns_keep_names_and_images_offscreen() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let mut state = AppState::test_default()?;
        state.images = ImageEngine::disabled_kitty(Arc::clone(&state.cfg));
        let url = MediaUrl::remote("https://example.com/detail-cover.png")?;
        state.images.insert_test_thumbnail(&url)?;
        let mut covered = with_name(song("covered"), "Alpha");
        covered.cover_url = Some(url.clone());
        let missing = with_name(song("missing"), "Beta");
        let mut later = with_name(song("later"), "Gamma");
        later.cover_url = Some(url.clone());
        let songs = [covered, missing, later];
        let albums = songs
            .iter()
            .map(|song| {
                Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, song.name.clone()))
                    .name(song.name.clone())
                    .cover_url(song.cover_url.clone())
                    .build()
            })
            .collect::<Vec<_>>();
        let list = ScrollList::new();
        let area = Rect::new(7, 4, 60, 5);
        for is_album in [false, true] {
            let render = |buf: &mut Buffer, motion| {
                let paint = ListPaint {
                    list: &list,
                    motion,
                    focus_permille: 1000,
                };
                if is_album {
                    draw_album_list(buf, area, &albums, paint, &state, &theme);
                } else {
                    draw_track_list(
                        buf,
                        area,
                        TrackList::Songs(&songs),
                        paint,
                        super::TrackColumns::new(false, true),
                        &state,
                        &theme,
                    );
                }
            };
            let mut stable = Buffer::empty(area);
            render(
                &mut stable,
                ScrollMotion::Advancing {
                    scrolloff: 0,
                    glide_ticks: 1,
                },
            );
            let name_x = (area.left()..area.right())
                .find(|&x| {
                    stable
                        .cell((x, area.y + 1))
                        .is_some_and(|c| c.symbol() == "A")
                })
                .ok_or_else(|| color_eyre::eyre::eyre!("缺少详情名称"))?;
            let cover_x = name_x - super::THUMBNAIL_COLUMNS - 1;
            for (row, name) in ["A", "B", "G"].into_iter().enumerate() {
                let y = area.y + 1 + u16::try_from(row)?;
                assert_eq!(
                    stable.cell((name_x, y)).map(ratatui::buffer::Cell::symbol),
                    Some(name)
                );
                assert_eq!(
                    stable
                        .cell((cover_x, y))
                        .is_some_and(|c| c.symbol().contains('\u{10EEEE}')),
                    row != 1,
                    "无封面的中间行不能挤掉后续封面"
                );
            }
            let mut frozen = Buffer::empty(area);
            render(&mut frozen, ScrollMotion::Frozen);
            assert!(
                frozen.content.iter().all(|c| !c.symbol().contains('\x1b')),
                "Frozen 不能输出图形控制序列"
            );
            for row in 1..=3 {
                for x in cover_x..cover_x + super::THUMBNAIL_COLUMNS {
                    assert_eq!(
                        frozen.cell((x, area.y + row)),
                        stable.cell((x, area.y + row)),
                        "离屏帧保留封面列下标、图片身份与行背景"
                    );
                }
            }
            assert_eq!(
                frozen
                    .cell((name_x, area.y + 1))
                    .map(ratatui::buffer::Cell::symbol),
                Some("A")
            );
        }
        Ok(())
    }
}
