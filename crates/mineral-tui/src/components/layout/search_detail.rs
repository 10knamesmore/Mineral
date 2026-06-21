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
use ratatui::widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table, Widget, Wrap};
use ratatui_image::picker::Picker;

use mineral_config::SweepStyle;
use mineral_model::{Album, Artist, Song};

use crate::components::layout::scroll_table::render_scroll_table;
use crate::components::layout::search_panel::join_artists;
use crate::components::layout::track_table::{self, TrackColumns};
use crate::components::layout::{cover, cover_image, description, detail_title};
use crate::render::theme::Theme;
use crate::runtime::scroll_list::{ScrollList, ScrollMotion};
use crate::runtime::state::{AppState, ArtistSection, DetailData, DetailFrame, EntityRef};

/// 缓动进度满值（千分比）。
const FULL: u32 = 1000;

/// 稳态实拍的视口推进语义(按 scrolloff + 缓动拍数);离屏合成 / 滑动期改用 [`ScrollMotion::Frozen`]。
fn advancing(state: &AppState) -> ScrollMotion {
    ScrollMotion::Advancing {
        scrolloff: state.scrolloff(),
        glide_ticks: state.list_glide_ticks(),
    }
}

/// 列表区渲染的滚动两件套:光标 + 视口态 + 本帧推进语义。穿过 body 渲染链时合并一束传,压参数个数。
#[derive(Clone, Copy)]
struct ListPaint<'a> {
    /// 该列表的光标 + 视口滚动态(取自栈顶帧)。
    list: &'a ScrollList,

    /// 本帧推进语义(稳态 Advancing / 离屏 Frozen)。
    motion: ScrollMotion,
}

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
        .title(detail_title::for_panel(state, area.width));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(kr) = state.channel_search.active_results() else {
        return;
    };
    if inner.height < 2 || inner.width == 0 {
        return;
    }
    match kr.detail.sweep_frames() {
        Some((from, to, eased, is_push)) => draw_sweep(
            frame,
            inner,
            SweepArgs {
                from,
                to,
                eased,
                is_push,
            },
            state,
            theme,
        ),
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
    // 稳态实拍:列表视口推进缓动(每帧恰一次)。
    draw_body(buf, body, dframe, state, theme, advancing(state));
}

/// 把一帧渲染到（离屏）Buffer：头图走程序化占位（滑动期不上真图），元数据 + 列表照画。
fn render_frame_to(
    buf: &mut Buffer,
    inner: Rect,
    dframe: &DetailFrame,
    state: &AppState,
    theme: &Theme,
) {
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
    // 离屏合成(下钻 / 返回滑动期):只读展示当前视口,不推进动画、不改滚动目标。
    draw_body(buf, body, dframe, state, theme, ScrollMotion::Frozen);
}

/// 下钻/返回滑动的合成参数：出发/目标帧 + 缓动进度 + 方向（打包避免 `draw_sweep` 参数过多）。
#[derive(Clone, Copy)]
struct SweepArgs<'a> {
    /// 出发帧（滑出）。
    from: &'a DetailFrame,

    /// 目标帧（滑入）。
    to: &'a DetailFrame,

    /// 缓动后进度（千分比）。
    eased: u16,

    /// 方向：`true` = 下钻右入、`false` = 返回左入。
    is_push: bool,
}

/// 下钻/返回滑动：出发帧与目标帧各渲染到离屏 Buffer，按 `eased` 列合成（Cover 风格：
/// push 目标从右覆盖、pop 目标从左覆盖）。
fn draw_sweep(
    frame: &mut Frame<'_>,
    inner: Rect,
    args: SweepArgs<'_>,
    state: &AppState,
    theme: &Theme,
) {
    let SweepArgs {
        from,
        to,
        eased,
        is_push,
    } = args;
    let mut from_buf = Buffer::empty(inner);
    let mut to_buf = Buffer::empty(inner);
    render_frame_to(&mut from_buf, inner, from, state, theme);
    render_frame_to(&mut to_buf, inner, to, state, theme);
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
            .and_then(|a| a.songs.get(dframe.list().sel()))
            .map_or("", |s| {
                s.album
                    .as_ref()
                    .map_or(s.name.as_str(), |al| al.name.as_str())
            }),
        ArtistSection::Albums => albums
            .as_ref()
            .and_then(|v| v.get(dframe.list().sel()))
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

/// 元数据区：上半固定 header（名 + 次行 + 计量，wrap 折行），下半是独立可滚动简介视口
/// （C-d/u/b/f 滚动，按 `\n` 多行渲染、溢出画滚动条）。两者间留一行视觉间隔。
fn draw_meta(buf: &mut Buffer, meta_a: Rect, dframe: &DetailFrame, theme: &Theme, show_back: bool) {
    let pad = Rect::new(
        meta_a.x.saturating_add(1),
        meta_a.y,
        meta_a.width.saturating_sub(1),
        meta_a.height,
    );
    if pad.width == 0 || pad.height == 0 {
        return;
    }
    let mut header = meta_lines(dframe, theme);
    if show_back {
        header.push(Line::from(Span::styled(
            "‹ Esc back",
            Style::new().fg(theme.peach),
        )));
    }
    // header 占其行数 + 1 行间隔；简介拿剩余高度（不足时 Layout 自动裁）。
    let head_h = u16::try_from(header.len()).unwrap_or(0).saturating_add(1);
    let [head_a, desc_a] =
        Layout::vertical([Constraint::Length(head_h), Constraint::Min(0)]).areas(pad);
    Widget::render(
        Paragraph::new(header).wrap(Wrap { trim: false }),
        head_a,
        buf,
    );
    description::draw_description(
        buf,
        desc_a,
        frame_description(dframe),
        dframe.description_scroll(),
        theme,
    );
}

/// 当前帧头部该展示的简介原文（歌曲取其所属专辑的、专辑/歌手取聚合 detail 的、歌单取自身的）；
/// 拿不到为空串（不渲染）。
fn frame_description(dframe: &DetailFrame) -> &str {
    match &dframe.entity {
        EntityRef::Song(_) => match &dframe.data {
            Some(DetailData::Album(a)) => &a.description,
            _ => "",
        },
        EntityRef::Album(_) => dframe.album_meta().map_or("", |a| a.description.as_str()),
        EntityRef::Artist(_) => dframe.artist_meta().map_or("", |a| a.description.as_str()),
        EntityRef::Playlist(p) => &p.description,
    }
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
            lines
        }
        EntityRef::Playlist(p) => vec![
            Line::from(Span::styled(p.name.clone(), name)),
            Line::from(Span::styled(format!("{} tracks", p.track_count), sub)),
        ],
    }
}

/// 主体：歌手帧画双区，其余画曲目列表。`motion` 透传给底层列表(稳态推进 / 离屏冻结)。
fn draw_body(
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
        _ => draw_skeleton(buf, body, theme),
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
    let [tabs, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(body);
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
        _ => draw_skeleton(buf, list, theme),
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
        let (src, src_c) = match style {
            // 新区从右覆盖：右 advance 列取 over。
            SweepStyle::Cover => {
                let split = w.saturating_sub(advance);
                if c < split {
                    (base, c)
                } else {
                    (over, c - split)
                }
            }
            // 整体左移 advance，新区从右补入。非穷尽（`#[non_exhaustive]`）→ 按 Push 兜底。
            SweepStyle::Push | _ => {
                if c + advance < w {
                    (base, c + advance)
                } else {
                    (over, c + advance - w)
                }
            }
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
        draw_skeleton(buf, area, theme);
        return;
    }
    let cols = cols.for_width(area.width);
    let rows = songs.iter().enumerate().map(|(idx, s)| {
        let loved = state.is_liked(s);
        let is_current = state.player.current.as_ref().is_some_and(|c| c.id == s.id);
        track_table::track_row(s, idx, loved, is_current, cols, theme)
    });
    let table = Table::new(rows, cols.widths())
        .header(track_table::header_row(cols, theme))
        .row_highlight_style(track_table::highlight_style(theme))
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
        draw_skeleton(buf, area, theme);
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
        let tracks = if a.track_count > 0 {
            with_commas(a.track_count)
        } else {
            String::new()
        };
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
        .row_highlight_style(track_table::highlight_style(theme))
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

/// 专辑卡片的 header 行:名(bold)+ 艺人 + 计量行(N tracks · 年 · 厂牌)。简介不在此，
/// 走独立可滚动视口（见 [`frame_description`]）。
///
/// album 帧与 song 帧共用同一套——歌曲的详情即其所属专辑,故选中歌时头部与「直接搜 album」
/// 看到的一致。
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

#[cfg(test)]
mod tests {
    use super::publish_year;

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
