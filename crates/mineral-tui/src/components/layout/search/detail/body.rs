//! Detail 面板栈顶帧的主体列表区:非歌手帧曲目表、歌手帧热门曲/专辑双区(Tab + 切区
//! 离屏合成),及两张表(曲目/专辑)的渲染。数据未到画骨架,选中行高亮随面板焦点度渐变。

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, Widget};

use mineral_config::SweepStyle;
use mineral_model::{Album, Song};

use crate::components::layout::shared::marquee::{MarqueeCtx, resolve_column_widths, row_marquee};
use crate::components::layout::shared::scroll_table::render_scroll_table;
use crate::components::layout::shared::text::display_width;
use crate::render::theme::Theme;
use crate::runtime::marquee::Slot;
use crate::runtime::scroll::list::{ScrollList, ScrollMotion};
use crate::runtime::state::{
    AppState, ArtistSection, DetailData, DetailFrame, EntityRef, SearchFocus,
};

use super::meta::{publish_year, with_commas};
use super::placeholder::{draw_empty, draw_loading, loading_glyph};
use super::sweep::{SweepLayer, copy_col, sweep_column};
use super::track_table::{self, TrackColumns, highlight_style};
use super::{FULL, split_artist_body};

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

/// 主体：歌手帧画双区，其余画曲目列表。`motion` 透传给底层列表(稳态推进 / 离屏冻结)。
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

/// 非歌手帧主体：曲目列表（数据未到画骨架）。album / song 帧曲目来自 `Album.songs`（同源
/// 专辑无需再列 album 列），歌单帧来自 `Tracks`（混源，多列出 album）。
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
            &a.songs,
            paint,
            TrackColumns::new(/*artist*/ true, /*album*/ false),
            state,
            theme,
        ),
        Some(DetailData::Tracks(songs)) => draw_track_list(
            buf,
            body,
            songs,
            paint,
            TrackColumns::new(/*artist*/ true, /*album*/ true),
            state,
            theme,
        ),
        // 数据未到货 → 旋转 loading(非空态)。
        _ => draw_loading(buf, body, loading_glyph(state), theme),
    }
}

/// 歌手帧主体：热门曲/专辑双区 Tab + 当前区列表。热门曲是该歌手的歌，artist 列冗余，
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

/// 画歌手某一区的列表：Top Songs 走曲目表（artist 列冗余，出 album 列）、Albums 走专辑表；
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
            &a.songs,
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
        ) => draw_album_list(buf, list, albs, paint, theme),
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

/// 歌手双区 Tab：当前区 accent 高亮，另一区暗调；右侧 `[ / ]` 切区提示。
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

/// 曲目表（♥/#/title/[artist]/[album]/len，带表头）：对齐 browse library 表风格。
/// `cols` 选中间列、按面板宽度降级；`list` 选中行整行高亮 + nvim 视口滚动,在播歌 `#` 列显 `♫`、
/// 已收藏显 `♥`。`motion` 定推进(稳态)/ 冻结(离屏)。
fn draw_track_list(
    buf: &mut Buffer,
    area: Rect,
    songs: &[Song],
    paint: ListPaint<'_>,
    cols: TrackColumns,
    state: &AppState,
    theme: &Theme,
) {
    if songs.is_empty() {
        // 已到货但 0 曲 → 静态空态(非 loading,数据已在手)。
        draw_empty(buf, area, "no tracks", theme);
        return;
    }
    let cols = cols.for_width(area.width);
    let widths = cols.widths();
    // 表格选中行的 fade 实际会被 row_highlight_style 整行 fg 盖掉(刻意保留整行
    // accent,见 MarqueeCtx::fade_to 注);fade_to 仍按其底色给,不误导插值方向。
    let marquee_ctx = MarqueeCtx::new(state, theme, /*fade_to*/ theme.surface0);
    let title_w = resolve_column_widths(
        area.width,
        &widths,
        display_width(track_table::HIGHLIGHT_SYMBOL),
    )
    .get(2)
    .copied()
    .unwrap_or(0);
    let sel = paint.list.sel();
    let rows = songs.iter().enumerate().map(|(idx, s)| {
        let loved = state.is_liked(s);
        let is_current = state.player.current.as_ref().is_some_and(|c| c.id == s.id);
        let marquee = row_marquee(
            idx == sel,
            &marquee_ctx,
            Slot::SearchDetailSelected,
            title_w,
        );
        track_table::track_row(s, idx, loved, is_current, cols, theme, marquee)
    });
    let table = Table::new(rows, widths)
        .header(track_table::header_row(cols, theme))
        .row_highlight_style(highlight_style(theme, paint.focus_permille))
        .highlight_symbol(track_table::HIGHLIGHT_SYMBOL);
    // 视口行数 = 区高 - 表头(无 block 边框,area 已是内容区)。
    let viewport = usize::from(area.height.saturating_sub(1));
    render_scroll_table(
        buf,
        area,
        table,
        paint.list,
        songs.len(),
        viewport,
        paint.motion,
    );
}

/// 专辑表（name/tracks/year/label，带表头）：artist Albums 区，`list` 选中行整行高亮（下钻入口）。
fn draw_album_list(
    buf: &mut Buffer,
    area: Rect,
    albums: &[Album],
    paint: ListPaint<'_>,
    theme: &Theme,
) {
    if albums.is_empty() {
        // 已到货但 0 专辑 → 静态空态。
        draw_empty(buf, area, "no albums", theme);
        return;
    }
    let meta = Style::new().fg(theme.overlay);
    let header = Row::new(vec![
        Cell::from("name"),
        Cell::from("tracks"),
        Cell::from("year"),
        Cell::from("label"),
    ])
    .style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));
    let rows = albums.iter().map(|a| {
        // 曲目数未知(搜索 / 投稿列表投影)画 `-`,别画 `0` 冒充空专辑;下钻 album_detail 回填真值。
        let tracks = a.track_count.map_or_else(|| "-".to_owned(), with_commas);
        let year = publish_year(a.publish_time_ms).map_or_else(String::new, |y| y.to_string());
        let label = a.company.as_deref().unwrap_or_default().to_owned();
        Row::new(vec![
            Cell::from(Span::styled(a.name.clone(), Style::new().fg(theme.text))),
            Cell::from(Span::styled(tracks, meta)),
            Cell::from(Span::styled(year, meta)),
            Cell::from(Span::styled(label, meta)),
        ])
    });
    let widths = [
        Constraint::Fill(3),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Fill(2),
    ];
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(highlight_style(theme, paint.focus_permille))
        .highlight_symbol(track_table::HIGHLIGHT_SYMBOL);
    // 视口行数 = 区高 - 表头(无 block 边框,area 已是内容区)。
    let viewport = usize::from(area.height.saturating_sub(1));
    render_scroll_table(
        buf,
        area,
        table,
        paint.list,
        albums.len(),
        viewport,
        paint.motion,
    );
}
