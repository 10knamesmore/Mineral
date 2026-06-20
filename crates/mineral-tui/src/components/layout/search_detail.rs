//! Search detail 面板：实体详情栈顶帧的页头式渲染——左正方头图 + 右元数据，其下曲目/
//! 专辑列表；歌手帧多一行热门曲/专辑双区 Tab。数据未到画占位骨架。
//!
//! 稳态直接画在主帧（头图走 covers 管线的真图）；下钻/返回滑动期，出发帧与目标帧各渲染到
//! 离屏 Buffer（头图走程序化占位、不上 kitty 真图，根治图像穿透），再按 sweep 进度列合成。

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Paragraph, Row, StatefulWidget, Table, TableState, Widget,
    Wrap,
};
use ratatui_image::picker::Picker;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use mineral_model::{Album, Artist, SearchKind, Song};

use crate::components::layout::search_panel::{format_duration, join_artists};
use crate::components::layout::{cover, cover_image};
use crate::render::theme::Theme;
use crate::runtime::state::{AppState, ArtistSection, DetailData, DetailFrame, EntityRef};

/// 缓动进度满值（千分比）。
const FULL: u32 = 1000;

/// 画 detail 面板：bordered 外框 + 当前栈顶帧。空结果/无栈画空框；滑动期走 sweep 合成。
///
/// # Params:
///   - `border_focused`: 边框是否高亮（焦点环滑动期由调用方置 `false`）
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    picker: &Picker,
    theme: &Theme,
    border_focused: bool,
) {
    let color = if border_focused {
        theme.accent
    } else {
        theme.overlay
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color))
        .border_type(BorderType::Rounded)
        .title(detail_title(state, area.width));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(kr) = state.channel_search.active_results() else {
        return;
    };
    if inner.height < 2 || inner.width == 0 {
        return;
    }
    match kr.detail.sweep_frames() {
        Some((from, to, eased, is_push)) => {
            draw_sweep(frame, inner, from, to, eased, is_push, theme)
        }
        None => {
            let Some(dframe) = kr.detail.current() else {
                return;
            };
            // 下钻帧(depth>0)头部显示「‹ Esc back」返回提示(spec 二级头部)。
            draw_frame_real(
                frame,
                inner,
                dframe,
                state,
                picker,
                theme,
                kr.detail.depth() > 0,
            );
        }
    }
}

/// 稳态一帧：头图走 covers 管线真图，元数据 + 列表画主帧。
fn draw_frame_real(
    frame: &mut Frame<'_>,
    inner: Rect,
    dframe: &DetailFrame,
    state: &AppState,
    picker: &Picker,
    theme: &Theme,
    show_back: bool,
) {
    let (head, body) = split_frame(inner);
    let is_artist = matches!(dframe.entity, EntityRef::Artist(_));
    let (cover_a, meta_a, right_a) = split_head(head, is_artist);
    cover_image::render_or_fallback(
        frame,
        cover_a,
        dframe.entity.cover(),
        state,
        picker,
        theme,
        header_seed(&dframe.entity),
    );
    // artist 帧右栏:当前列表选中项封面(随 [ ] 切区 / 光标移动;图未到 / 滚动期走占位)。
    if let Some(right_a) = right_a {
        let sel_cover = dframe.selected_cover();
        cover_image::render_or_fallback(
            frame,
            right_a,
            sel_cover,
            state,
            picker,
            theme,
            selected_seed(dframe),
        );
    }
    let buf = frame.buffer_mut();
    draw_meta(buf, meta_a, dframe, theme, show_back);
    draw_body(buf, body, dframe, theme);
}

/// 把一帧渲染到（离屏）Buffer：头图走程序化占位（滑动期不上真图），元数据 + 列表照画。
fn render_frame_to(buf: &mut Buffer, inner: Rect, dframe: &DetailFrame, theme: &Theme) {
    let (head, body) = split_frame(inner);
    let is_artist = matches!(dframe.entity, EntityRef::Artist(_));
    let (cover_a, meta_a, right_a) = split_head(head, is_artist);
    if cover_a.width > 0 {
        cover::render_to(buf, cover_a, header_seed(&dframe.entity), theme);
    }
    if let Some(right_a) = right_a {
        cover::render_to(buf, right_a, selected_seed(dframe), theme);
    }
    draw_meta(buf, meta_a, dframe, theme, /*show_back*/ false);
    draw_body(buf, body, dframe, theme);
}

/// 下钻/返回滑动：出发帧与目标帧各渲染到离屏 Buffer，按 `eased` 列合成（Cover 风格：
/// push 目标从右覆盖、pop 目标从左覆盖）。
fn draw_sweep(
    frame: &mut Frame<'_>,
    inner: Rect,
    from: &DetailFrame,
    to: &DetailFrame,
    eased: u16,
    is_push: bool,
    theme: &Theme,
) {
    let mut from_buf = Buffer::empty(inner);
    let mut to_buf = Buffer::empty(inner);
    render_frame_to(&mut from_buf, inner, from, theme);
    render_frame_to(&mut to_buf, inner, to, theme);
    let w = inner.width;
    let advance = u16::try_from(u32::from(w) * u32::from(eased) / FULL)
        .unwrap_or(w)
        .min(w);
    let buf = frame.buffer_mut();
    for c in 0..w {
        let (src, src_c) = if is_push {
            // 目标从右覆盖：右 advance 列取目标帧。
            let split = w.saturating_sub(advance);
            if c < split {
                (&from_buf, c)
            } else {
                (&to_buf, c - split)
            }
        } else if c < advance {
            // 目标从左覆盖：左 advance 列取目标帧。
            (&to_buf, c)
        } else {
            (&from_buf, c)
        };
        copy_col(buf, inner, src, c, src_c);
    }
}

/// 把离屏 `src` 的第 `src_c` 列（相对 `area`）整列搬到 `dst` 的第 `dst_c` 列。
fn copy_col(dst: &mut Buffer, area: Rect, src: &Buffer, dst_c: u16, src_c: u16) {
    let dx = area.x.saturating_add(dst_c);
    let sx = area.x.saturating_add(src_c);
    for ry in area.y..area.y.saturating_add(area.height) {
        if let Some(cell) = src.cell((sx, ry)) {
            let cell = cell.clone();
            if let Some(slot) = dst.cell_mut((dx, ry)) {
                *slot = cell;
            }
        }
    }
}

/// 头部占面板高 ~45%，让左右两张正方 cover 拿到够大的 Rect；夹下限保矮面板仍有基本头部、
/// 夹上限给列表留可视行。
fn split_frame(inner: Rect) -> (Rect, Rect) {
    let head_h = (inner.height * 9 / 20)
        .max(6)
        .min(inner.height.saturating_sub(3));
    let [head, body] =
        Layout::vertical([Constraint::Length(head_h), Constraint::Min(0)]).areas(inner);
    (head, body)
}

/// 头部横分：左正方头图 + 中元数据；`with_right` 时右侧再切一栏放选中项 cover（artist 帧）。
///
/// 窄面板响应式降级：宽度容不下三栏退两栏、容不下两栏退纯 meta（左 cover 栏宽置 0，由调用方
/// 按 `width == 0` 跳过绘制）。
fn split_head(head: Rect, with_right: bool) -> (Rect, Rect, Option<Rect>) {
    // 单张 cover 栏的最小可视宽度断点：窄于此画出来只是无意义细条，宁可砍掉让位 meta。
    const MIN_COVER_W: u16 = 8;
    if with_right && head.width >= MIN_COVER_W * 3 {
        let [cover_a, meta_a, right_a] = Layout::horizontal([
            Constraint::Percentage(28),
            Constraint::Min(1),
            Constraint::Percentage(28),
        ])
        .areas(head);
        (cover_a, meta_a, Some(right_a))
    } else if head.width >= MIN_COVER_W * 2 {
        let [cover_a, meta_a] =
            Layout::horizontal([Constraint::Percentage(30), Constraint::Min(1)]).areas(head);
        (cover_a, meta_a, None)
    } else {
        (Rect::new(head.x, head.y, 0, head.height), head, None)
    }
}

/// artist 帧当前 section 选中项的 fallback seed（歌用所属专辑名、专辑用专辑名）；
/// 非 artist / 数据未到 → `""`。封面本身走 [`DetailFrame::selected_cover`]。
fn selected_seed(dframe: &DetailFrame) -> &str {
    let (EntityRef::Artist(_), Some(DetailData::Artist { detail, albums })) =
        (&dframe.entity, &dframe.data)
    else {
        return "";
    };
    match dframe.section {
        // 歌的头图 = 其所属专辑封面；seed 用专辑名（同专辑共享一张占位）。
        ArtistSection::Hot => detail
            .as_ref()
            .and_then(|a| a.songs.get(dframe.list_sel))
            .map_or("", |s| {
                s.album
                    .as_ref()
                    .map_or(s.name.as_str(), |al| al.name.as_str())
            }),
        ArtistSection::Albums => albums
            .as_ref()
            .and_then(|v| v.get(dframe.list_sel))
            .map_or("", |al| al.name.as_str()),
    }
}

/// 头图程序化 fallback 的 seed：歌曲用所属专辑名（同专辑共享一张）、其余用自身名。
fn header_seed(entity: &EntityRef) -> &str {
    match entity {
        EntityRef::Song(s) => s
            .album
            .as_ref()
            .map_or(s.name.as_str(), |a| a.name.as_str()),
        other => other.name(),
    }
}

/// 元数据区：名（bold）+ 次行（艺人/粉丝/计量）+ artist 简介（wrap 折行填充）。
fn draw_meta(buf: &mut Buffer, meta_a: Rect, dframe: &DetailFrame, theme: &Theme, show_back: bool) {
    let pad = Rect::new(
        meta_a.x.saturating_add(1),
        meta_a.y,
        meta_a.width.saturating_sub(1),
        meta_a.height,
    );
    let mut lines = meta_lines(dframe, theme);
    if show_back {
        lines.push(Line::from(Span::styled(
            "‹ Esc back",
            Style::new().fg(theme.peach),
        )));
    }
    // wrap 让 artist 长简介按 meta 宽折行填充（其余短行不受影响），超出区域高度自动裁。
    Widget::render(Paragraph::new(lines).wrap(Wrap { trim: false }), pad, buf);
}

/// 元数据行内容（按实体类型）。artist 帧的计数/简介取 fetch 回来的完整 detail——结果列那份
/// `entity` 来自搜索端点，无 `album_count`/`song_count`/`description`；未到货退回 `entity`。
fn meta_lines(dframe: &DetailFrame, theme: &Theme) -> Vec<Line<'static>> {
    let name = Style::new().fg(theme.text).add_modifier(Modifier::BOLD);
    let sub = Style::new().fg(theme.subtext);
    let dim = Style::new().fg(theme.overlay);
    match &dframe.entity {
        EntityRef::Song(s) => match &dframe.data {
            // 歌曲的详情即其所属专辑:专辑详情到货就照专辑卡片画(名/艺人/计量/简介),与
            // 「直接搜 album」的详情同一套;未到货退回歌名 + 艺人占位（专辑名作标题）。
            Some(DetailData::Album(a)) => album_card_lines(a, theme),
            _ => {
                let title = s
                    .album
                    .as_ref()
                    .map_or_else(|| s.name.clone(), |a| a.name.clone());
                vec![
                    Line::from(Span::styled(title, name)),
                    Line::from(Span::styled(join_artists(&s.artists), sub)),
                ]
            }
        },
        EntityRef::Album(entity_a) => {
            // 整份用 album_meta 选定的 album（fetch 完整 detail 优先、entity 占位兜底）。
            album_card_lines(dframe.album_meta().unwrap_or(&**entity_a), theme)
        }
        EntityRef::Artist(entity_a) => {
            // 整份用 artist_meta 选定的 artist（fetch 完整 detail 优先、entity 占位兜底）；
            // 渲染层只读字段，不关心数据来自哪个端点——聚合已在 channel 边缘完成。
            let a = dframe.artist_meta().unwrap_or(&**entity_a);
            let mut lines = vec![
                Line::from(Span::styled(a.name.clone(), name)),
                Line::from(Span::styled(
                    format!("{} fans", with_commas(a.follower_count)),
                    sub,
                )),
            ];
            if let Some(counts) = artist_counts(a) {
                lines.push(Line::from(Span::styled(counts, dim)));
            }
            if !a.description.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(a.description.clone(), dim)));
            }
            lines
        }
        EntityRef::Playlist(p) => vec![
            Line::from(Span::styled(p.name.clone(), name)),
            Line::from(Span::styled(format!("{} tracks", p.track_count), sub)),
            Line::from(Span::styled(truncate(&p.description, 64), dim)),
        ],
    }
}

/// 主体：歌手帧画双区，其余画曲目列表。
fn draw_body(buf: &mut Buffer, body: Rect, dframe: &DetailFrame, theme: &Theme) {
    match &dframe.entity {
        EntityRef::Artist(_) => draw_artist_body(buf, body, dframe, theme),
        _ => draw_track_body(buf, body, dframe, theme),
    }
}

/// 非歌手帧主体：曲目列表（数据未到画骨架）。album / song 帧曲目来自 `Album.songs`,
/// 歌单帧来自 `Tracks`。
fn draw_track_body(buf: &mut Buffer, body: Rect, dframe: &DetailFrame, theme: &Theme) {
    match &dframe.data {
        Some(DetailData::Album(a)) => draw_track_list(buf, body, &a.songs, dframe.list_sel, theme),
        Some(DetailData::Tracks(songs)) => {
            draw_track_list(buf, body, songs, dframe.list_sel, theme)
        }
        _ => draw_skeleton(buf, body, theme),
    }
}

/// 歌手帧主体：热门曲/专辑双区 Tab + 当前区列表。
fn draw_artist_body(buf: &mut Buffer, body: Rect, dframe: &DetailFrame, theme: &Theme) {
    if body.height < 2 {
        return;
    }
    let [tabs, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(body);
    draw_artist_tabs(buf, tabs, dframe.section, theme);
    match (dframe.section, &dframe.data) {
        (
            ArtistSection::Hot,
            Some(DetailData::Artist {
                detail: Some(a), ..
            }),
        ) => {
            draw_track_list(buf, list, &a.songs, dframe.list_sel, theme);
        }
        (
            ArtistSection::Albums,
            Some(DetailData::Artist {
                albums: Some(albs), ..
            }),
        ) => {
            draw_album_list(buf, list, albs, dframe.list_sel, theme);
        }
        _ => draw_skeleton(buf, list, theme),
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

/// 曲目列表（title · len）：当前 `sel` 行整行高亮。
fn draw_track_list(buf: &mut Buffer, area: Rect, songs: &[Song], sel: usize, theme: &Theme) {
    if songs.is_empty() {
        draw_skeleton(buf, area, theme);
        return;
    }
    let rows = songs.iter().map(|s| {
        Row::new(vec![
            Cell::from(Span::styled(s.name.clone(), Style::new().fg(theme.text))),
            Cell::from(Span::styled(
                format_duration(s.duration_ms),
                Style::new().fg(theme.overlay),
            )),
        ])
    });
    let table = Table::new(rows, [Constraint::Fill(1), Constraint::Length(5)])
        .row_highlight_style(highlight(theme))
        .highlight_symbol("▌ ");
    let mut st = TableState::default().with_selected(Some(sel.min(songs.len().saturating_sub(1))));
    StatefulWidget::render(table, area, buf, &mut st);
}

/// 专辑列表（name）：当前 `sel` 行整行高亮（下钻入口）。
fn draw_album_list(buf: &mut Buffer, area: Rect, albums: &[Album], sel: usize, theme: &Theme) {
    if albums.is_empty() {
        draw_skeleton(buf, area, theme);
        return;
    }
    let rows = albums.iter().map(|a| {
        Row::new(vec![Cell::from(Span::styled(
            a.name.clone(),
            Style::new().fg(theme.text),
        ))])
    });
    let table = Table::new(rows, [Constraint::Fill(1)])
        .row_highlight_style(highlight(theme))
        .highlight_symbol("▌ ");
    let mut st = TableState::default().with_selected(Some(sel.min(albums.len().saturating_sub(1))));
    StatefulWidget::render(table, area, buf, &mut st);
}

/// 列表选中行整行高亮样式。
fn highlight(theme: &Theme) -> Style {
    Style::new()
        .bg(theme.surface0)
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD)
}

/// 数据未到的占位骨架（几行暗调虚线）。
fn draw_skeleton(buf: &mut Buffer, area: Rect, theme: &Theme) {
    let style = Style::new().fg(theme.overlay).add_modifier(Modifier::DIM);
    for row in 0..area.height.min(4) {
        let y = area.y.saturating_add(row);
        Widget::render(
            Paragraph::new(Line::from(Span::styled("  ┄┄┄┄┄┄┄┄", style))),
            Rect::new(area.x, y, area.width, 1),
            buf,
        );
    }
}

/// u64 千分位：`8900000` → `8,900,000`（detail 头部宽，展示完整数而非缩写）。
fn with_commas(n: u64) -> String {
    let s = n.to_string();
    let len = s.chars().count();
    let mut out = String::with_capacity(len + len / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// 专辑卡片的元数据行:名(bold)+ 艺人 + 计量行(N tracks · 年 · 厂牌)+ 简介。
///
/// album 帧与 song 帧共用同一套——歌曲的详情即其所属专辑,故选中歌时头部与「直接搜 album」
/// 看到的一致(简介只有详情端点给,未到货时为空不显示)。
fn album_card_lines(a: &Album, theme: &Theme) -> Vec<Line<'static>> {
    let name = Style::new().fg(theme.text).add_modifier(Modifier::BOLD);
    let sub = Style::new().fg(theme.subtext);
    let dim = Style::new().fg(theme.overlay);
    let mut lines = vec![
        Line::from(Span::styled(a.name.clone(), name)),
        Line::from(Span::styled(join_artists(&a.artists), sub)),
    ];
    if let Some(meta) = album_meta_line(a) {
        lines.push(Line::from(Span::styled(meta, dim)));
    }
    if !a.description.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(a.description.clone(), dim)));
    }
    lines
}

/// album 计量行 `N tracks · 2015 · 厂牌`（缺哪个省哪个；全缺 → `None`）。
fn album_meta_line(a: &Album) -> Option<String> {
    let mut parts = Vec::<String>::new();
    if a.track_count > 0 {
        parts.push(format!("{} tracks", with_commas(a.track_count)));
    }
    if let Some(year) = publish_year(a.publish_time_ms) {
        parts.push(year.to_string());
    }
    if let Some(company) = a.company.as_ref().filter(|c| !c.is_empty()) {
        parts.push(company.clone());
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// epoch 毫秒 → 发行年份；`<= 0`（未知）或换算失败 → `None`。
///
/// netease `publishTime` 是**北京零点**对齐的时间戳,故按 +8 偏移读年份——直接用 UTC 会让
/// 北京 1 月 1 日发行的专辑落到上一年 12 月 31 日、年份少算一年。
fn publish_year(ms: i64) -> Option<i32> {
    if ms <= 0 {
        return None;
    }
    let beijing = time::UtcOffset::from_hms(8, 0, 0).ok()?;
    let dt = time::OffsetDateTime::from_unix_timestamp(ms / 1000).ok()?;
    Some(dt.to_offset(beijing).year())
}

/// artist 计数行 `N albums · M songs`；两者皆 `None` → `None`（缺哪个省哪个）。
fn artist_counts(a: &Artist) -> Option<String> {
    let mut parts = Vec::<String>::new();
    if let Some(n) = a.album_count {
        parts.push(format!("{} albums", with_commas(n)));
    }
    if let Some(n) = a.song_count {
        parts.push(format!("{} songs", with_commas(n)));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// 截断到 `max` 个字符并补省略号（按 char 计，避免切断多字节）。
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// detail 顶栏 title：当前结果集的详情栈 breadcrumb；无结果 / 空栈回退固定 `detail`。
/// `width` 是面板外框宽，扣掉圆角边框占位后交给 [`frame_title`] 按显示宽度截断。
fn detail_title(state: &AppState, width: u16) -> String {
    let budget = width.saturating_sub(4);
    match state.channel_search.active_results() {
        Some(kr) => frame_title(&kr.detail.title_crumbs(), budget),
        None => "detail".to_owned(),
    }
}

/// 详情栈 breadcrumb → 顶栏 title 文案，按显示宽度 `max_width` 截断。
///
/// root 单帧 → `图标 单数类型 · 名`；下钻多帧 → 各帧 `图标 名` 用 ` › ` 链接（图标代类型）。
/// 超宽时优先截最左祖先名、保当前帧名完整；当前帧那一节自身放不下才截它补 `…`。空链 → `detail`。
///
/// # Params:
///   - `crumbs`: 栈帧链（root→top）的 `(类型, 名)`
///   - `max_width`: 可用显示宽度（列）
///
/// # Return:
///   组装并按宽度截断后的 title 文案。
fn frame_title(crumbs: &[(SearchKind, &str)], max_width: u16) -> String {
    let Some((last, ancestors)) = crumbs.split_last() else {
        return "detail".to_owned();
    };
    let (last_kind, last_name) = *last;
    if ancestors.is_empty() {
        let prefix = format!("{} {} · ", last_kind.icon(), last_kind.singular());
        return fit_prefixed(&prefix, last_name, max_width);
    }
    let last_seg = format!("{} {}", last_kind.icon(), last_name);
    let head = ancestors
        .iter()
        .map(|(k, n)| format!("{} {}", k.icon(), n))
        .collect::<Vec<String>>()
        .join(" › ");
    let sep = " › ";
    let full = format!("{head}{sep}{last_seg}");
    if display_width(&full) <= max_width {
        return full;
    }
    let reserve = display_width(&last_seg).saturating_add(display_width(sep));
    if reserve >= max_width {
        // 连当前帧那一节都放不下：退化为只截当前帧名（保图标）。
        let prefix = format!("{} ", last_kind.icon());
        return fit_prefixed(&prefix, last_name, max_width);
    }
    let head_trunc = truncate_to_width(&head, max_width.saturating_sub(reserve));
    format!("{head_trunc}{sep}{last_seg}")
}

/// `前缀 + 名` 放不下时只截名补 `…`（保前缀）；前缀本身就超宽则整体截断兜底。
fn fit_prefixed(prefix: &str, name: &str, max_width: u16) -> String {
    let full = format!("{prefix}{name}");
    if display_width(&full) <= max_width {
        return full;
    }
    let pw = display_width(prefix);
    if pw < max_width {
        let name = truncate_to_width(name, max_width.saturating_sub(pw));
        return format!("{prefix}{name}");
    }
    truncate_to_width(&full, max_width)
}

/// 按显示宽度截断到 `max_width`，截掉则补 `…`（占 1 列）；本就够宽原样返回。
fn truncate_to_width(s: &str, max_width: u16) -> String {
    if display_width(s) <= max_width {
        return s.to_owned();
    }
    let budget = max_width.saturating_sub(1); // 给省略号留 1 列
    let mut acc = 0u16;
    let mut out = String::new();
    for ch in s.chars() {
        let w = char_width(ch);
        if acc.saturating_add(w) > budget {
            break;
        }
        acc = acc.saturating_add(w);
        out.push(ch);
    }
    out.push('…');
    out
}

/// 字符串显示宽度（CJK 双宽）；溢出 u16 夹到 MAX。
fn display_width(s: &str) -> u16 {
    u16::try_from(UnicodeWidthStr::width(s)).unwrap_or(u16::MAX)
}

/// 单字符显示宽度（控制字符按 0）。
fn char_width(ch: char) -> u16 {
    u16::try_from(UnicodeWidthChar::width(ch).unwrap_or(0)).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use mineral_model::SearchKind;

    use super::{frame_title, publish_year};

    /// root 帧（单节）：`图标 单数类型 · 名`。
    #[test]
    fn title_root_frame_is_kind_and_name() {
        assert_eq!(
            frame_title(&[(SearchKind::Album, "范特西")], /*max_width*/ 40),
            "◉ album · 范特西"
        );
        assert_eq!(
            frame_title(&[(SearchKind::Playlist, "Chill")], 40),
            "▤ playlist · Chill"
        );
    }

    /// 下钻帧（多节）：图标代类型、名用 ` › ` 链接，宽度够时全展开。
    #[test]
    fn title_breadcrumb_joins_crumbs() {
        assert_eq!(
            frame_title(
                &[
                    (SearchKind::Artist, "周杰伦"),
                    (SearchKind::Album, "范特西")
                ],
                40,
            ),
            "✦ 周杰伦 › ◉ 范特西"
        );
    }

    /// 超宽：按显示宽度截祖先名（CJK 双宽）、当前帧名保持完整。
    #[test]
    fn title_breadcrumb_truncates_ancestor_keeps_current() {
        // 全长 "✦ 周杰伦 › ◉ 范特西" 显示宽 19；给 17 容不下 → 截祖先到 "✦ 周…"。
        assert_eq!(
            frame_title(
                &[
                    (SearchKind::Artist, "周杰伦"),
                    (SearchKind::Album, "范特西")
                ],
                17,
            ),
            "✦ 周… › ◉ 范特西"
        );
    }

    /// 当前帧名自己都放不下时：截当前帧名补省略号（祖前缀保留图标/类型词）。
    #[test]
    fn title_root_truncates_long_name() {
        // 前缀 "◉ album · " 宽 10；给 14 → 名字预算 4 → "范特西精选" 截成 "范…"。
        assert_eq!(
            frame_title(&[(SearchKind::Album, "范特西精选")], 14),
            "◉ album · 范…"
        );
    }

    /// 空链回退固定 `detail`（无实体可标）。
    #[test]
    fn title_empty_falls_back() {
        assert_eq!(frame_title(&[], 40), "detail");
    }

    /// 发行年份按北京 +8 偏移读：北京 1 月 1 日发行的专辑不能因 UTC 落到上一年。
    #[test]
    fn publish_year_uses_beijing_offset() {
        // 2015-09-26 00:00 北京（= 2015-09-25 16:00 UTC）→ 2015（两边同年,基线）。
        assert_eq!(publish_year(1_443_196_800_000), Some(2015));
        // 2020-01-01 00:00 北京（= 2019-12-31 16:00 UTC）→ 2020；UTC 读会错成 2019。
        assert_eq!(
            publish_year(1_577_808_000_000),
            Some(2020),
            "北京跨年不少算一年"
        );
        // 未知发行时间。
        assert_eq!(publish_year(0), None);
        assert_eq!(publish_year(-1), None);
    }
}
