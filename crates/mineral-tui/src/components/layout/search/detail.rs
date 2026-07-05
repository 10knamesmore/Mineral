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
use mineral_model::{Album, Song};

mod description;
mod meta;
mod placeholder;
mod sweep;
mod title;
mod track_table;

use crate::components::layout::shared::cover_image;
use crate::components::layout::shared::scroll_table::render_scroll_table;
use crate::render::theme::Theme;
use crate::runtime::scroll::list::{ScrollList, ScrollMotion};
use crate::runtime::state::{AppState, ArtistSection, DetailData, DetailFrame, EntityRef};

use self::meta::{album_card_lines, artist_counts, publish_year, with_commas};
use self::placeholder::{draw_delimiter, draw_empty, draw_loading, loading_glyph};
use self::sweep::{SweepLayer, copy_col, sweep_column};
use self::track_table::TrackColumns;
use super::panel::join_artists;

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
    let results = state.channel_search.active_results();
    let mut block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(color))
        .border_type(BorderType::Rounded)
        .title(title::for_panel(state, area.width));
    // 左下角位置标:当前栈顶帧当前区列表 ` n / total `(数据未到 len 0 不显)。detail 非分页
    // (一次拉全),故无 results 列那种 `+`。
    if let Some(dframe) = results.and_then(|kr| kr.detail.current()) {
        let len = dframe.list_len();
        if len > 0 {
            block = block.title_bottom(
                Line::from(detail_position_label(dframe.list().sel(), len))
                    .style(Style::new().fg(theme.overlay)),
            );
        }
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(kr) = results else {
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
    let (head, delim, body) = split_frame(inner);
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
    draw_delimiter(buf, delim, theme);
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
    let (head, delim, body) = split_frame(inner);
    let is_artist = matches!(dframe.entity, EntityRef::Artist(_));
    let (cover_a, meta_a, right_a) = split_head(head, is_artist);
    if cover_a.width > 0 {
        cover_image::render_morph_to(
            buf,
            cover_a,
            dframe.entity.cover(),
            state,
            theme,
            header_seed(&dframe.entity),
        );
    }
    if let Some(right_a) = right_a {
        cover_image::render_morph_to(
            buf,
            right_a,
            dframe.selected_cover(),
            state,
            theme,
            selected_seed(dframe),
        );
    }
    draw_meta(buf, meta_a, dframe, theme, /*show_back*/ false);
    draw_delimiter(buf, delim, theme);
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

/// 下钻/返回滑动：出发帧与目标帧各渲染到离屏 Buffer，按 `eased` 列合成。风格随配置
/// `view_sweep`（经 [`sweep_column`]），与歌手双区切换 / 左栏视图切换同款；方向由 `is_push`
/// 定（下钻目标右入、返回左入）。
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
    let style = *state.cfg.tui().animation().view_sweep();
    let buf = frame.buffer_mut();
    for c in 0..w {
        let (src, src_c) = match sweep_column(style, is_push, c, w, advance) {
            (SweepLayer::From, src_c) => (&from_buf, src_c),
            (SweepLayer::To, src_c) => (&to_buf, src_c),
        };
        copy_col(buf, inner, src, c, src_c);
    }
}

/// 纵分三段:头部(~45%,左右两张正方 cover) + 1 行 meta↔list 分隔 + 列表 body。
/// 夹下限保矮面板仍有基本头部、夹上限给分隔 + 列表留可视行。
fn split_frame(inner: Rect) -> (Rect, Rect, Rect) {
    let head_h = (inner.height * 9 / 20)
        .max(6)
        .min(inner.height.saturating_sub(4));
    let [head, delim, body] = Layout::vertical([
        Constraint::Length(head_h),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);
    (head, delim, body)
}

/// 歌手帧主体再分：顶 1 行双区 Tab + 其下列表区。抽出供渲染与 [`detail_list_area`] 共用，
/// 锚点与渲染走同一处几何。
fn split_artist_body(body: Rect) -> (Rect, Rect) {
    let [tabs, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(body);
    (tabs, list)
}

/// 当前栈顶帧「列表区」相对面板内区 `inner` 的屏幕矩形：去头部，歌手帧再去 Tab 行。
/// 渲染端与行级菜单锚点共用此一处几何——菜单贴的行恒等于渲染出的行。该列表区**无**自己
/// 的 block 边框（边框归整个 detail 面板），故其视口数学（区高 − 表头一行）与 browse 带
/// 边框面板不同，锚点侧据此还原行 y。
///
/// # Params:
///   - `inner`: detail 面板去外框后的内区
///   - `is_artist`: 是否歌手帧（多减一行 Tab）
pub(crate) fn detail_list_area(inner: Rect, is_artist: bool) -> Rect {
    let (_head, _delim, body) = split_frame(inner);
    if is_artist {
        split_artist_body(body).1
    } else {
        body
    }
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

/// detail 面板左下角 ` n / total `(1-based 当前位 / 当前区列表长度)。调用方已保证 `total != 0`。
///
/// # Params:
///   - `sel`: 0-based 选中行
///   - `total`: 当前区列表长度
fn detail_position_label(sel: usize, total: usize) -> String {
    format!(" {} / {total} ", sel.saturating_add(1).min(total))
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::{detail_list_area, split_frame};

    /// detail_list_area：非歌手帧 = split_frame 的 body 整块；歌手帧再去顶部 1 行 Tab。
    /// 这是渲染端与行级菜单锚点共用的几何契约——两侧据此对齐。
    #[test]
    fn detail_list_area_drops_head_and_artist_tab() {
        let inner = Rect::new(0, 0, 40, 30);
        let (_head, _delim, body) = split_frame(inner);
        assert_eq!(
            detail_list_area(inner, /*is_artist*/ false),
            body,
            "非歌手帧 = 整块 body"
        );
        let artist = detail_list_area(inner, /*is_artist*/ true);
        assert_eq!(artist.y, body.y + 1, "歌手帧列表区下移 1 行(让出 Tab)");
        assert_eq!(artist.height, body.height - 1, "歌手帧列表区少 1 行");
        assert_eq!(
            (artist.x, artist.width),
            (body.x, body.width),
            "横向与 body 同"
        );
    }
}
