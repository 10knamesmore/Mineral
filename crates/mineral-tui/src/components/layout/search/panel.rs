//! Search 布局态面板渲染:token prompt 输入行 + 结果列。

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table};

use unicode_width::UnicodeWidthStr;

use mineral_model::ArtistRef;
use mineral_task::SearchPayload;

use super::detail::highlight_style;
use crate::components::layout::shared::marquee::{MarqueeCtx, resolve_column_widths};
use crate::components::layout::shared::scroll_table::render_scroll_table;
use crate::components::layout::shared::spinner;
use crate::components::layout::shared::text::alias_span;
use crate::components::popup::{MenuItem, Placement, PopMenu, render_overlay};
use crate::render::cursor::cursor_spans;
use crate::render::theme::{Theme, resolve_source_color};
use crate::runtime::format::format_ms_opt;
use crate::runtime::marquee::Slot;
use crate::runtime::scroll::list::ScrollMotion;
use crate::runtime::state::{AppState, PromptSegment, SearchFocus, SearchPage, SearchSession};

/// 面板边框样式:焦点态 accent 高亮,否则 overlay 暗调(spec §1.2 当前焦点面板边框高亮)。
fn border_style(focused: bool, theme: &Theme) -> Style {
    let color = if focused { theme.accent } else { theme.overlay };
    Style::new().fg(color)
}

/// 画 token prompt 输入行:`[source chip] [kind chip] query`。
///
/// source chip 颜色经 [`resolve_source_color`] 按源名从配置落地(不 match source,
/// 插件 source 自动正确)。无可搜索 source(`current()` 为 `None`)画空态提示。持焦的 chip 段亮底,
/// query 段持焦才显光标块。
///
/// # Params:
///   - `rs`: channel 搜索子域(读当前 source / 会话 query / kind / 段焦点)
///   - `border_focused`: 边框是否高亮(焦点环滑动期由调用方置 `false`,改由浮动环表达高亮)
pub fn draw_prompt(
    frame: &mut Frame<'_>,
    area: Rect,
    rs: &SearchPage,
    theme: &Theme,
    sources: &mineral_config::SourcesConfig,
    border_focused: bool,
) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(border_style(border_focused, theme))
        .border_type(BorderType::Rounded)
        .title("search");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let (Some(tokens), Some(session)) = (prompt_tokens(area, rs), rs.current()) else {
        let hint = Span::styled("no searchable source", Style::new().fg(theme.overlay));
        frame.render_widget(Paragraph::new(Line::from(hint)), inner);
        return;
    };
    let focus = rs.prompt_focus();
    // source chip:focus 时 accent 反色药丸(暗字,最跳);否则 source 色暗染底 + source 色字(身份)。
    if let (Some(rect), Some(source)) = (tokens.source, rs.source) {
        let source_color = resolve_source_color(theme, sources, source);
        let (bg, fg) = if focus == Some(PromptSegment::Source) {
            (theme.accent, theme.crust)
        } else {
            (dark_tint(source_color, theme), source_color)
        };
        draw_chip(frame, rect, source.label(), fg, bg);
    }
    // kind chip:focus 时 accent 反色药丸;否则中性 surface0 底 + subtext。
    let (kbg, kfg) = if focus == Some(PromptSegment::Kind) {
        (theme.accent, theme.crust)
    } else {
        (theme.surface0, theme.subtext)
    };
    draw_chip(frame, tokens.kind, session.kind.label(), kfg, kbg);
    // query:光标块仅在 query 段持焦时显示(在 chip 段 / 离开 prompt 时只画文本)。
    draw_query(
        frame,
        tokens.query,
        session,
        theme,
        focus == Some(PromptSegment::Query),
    );
}

/// token prompt 行内各段的 1-row 子矩形:chip 背景填充(渲染）与 chip 下拉锚定（定位）
/// 读同一份几何,保证下拉贴在对应 chip 正下方而非整行输入框。
pub(crate) struct PromptTokens {
    /// source chip 矩形(无可搜索 source 时 `None`)。
    pub(crate) source: Option<Rect>,

    /// kind chip 矩形。
    pub(crate) kind: Rect,

    /// chip 之后的 query 输入区(光标块在此)。
    pub(crate) query: Rect,
}

/// 算 token prompt 各子矩形(无当前会话 → `None`,空态由调用方画提示)。
///
/// # Params:
///   - `area`: prompt 外框矩形(含边框;与 [`draw_prompt`] 同一 `area`)
///   - `rs`: channel 搜索子域(读当前源 / kind)
pub(crate) fn prompt_tokens(area: Rect, rs: &SearchPage) -> Option<PromptTokens> {
    let session = rs.current()?;
    let inner = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .inner(area);
    if inner.width == 0 || inner.height == 0 {
        return None;
    }
    let y = inner.y;
    let right = inner.right();
    let gap = 1u16;
    let mut x = inner.x;
    let mut take = |w: u16| -> Rect {
        let rect = Rect::new(x.min(right), y, w.min(right.saturating_sub(x)), 1);
        x = x.saturating_add(w).saturating_add(gap);
        rect
    };
    let source = rs.source.map(|src| take(chip_width(src.label())));
    let kind = take(chip_width(session.kind.label()));
    let query = Rect::new(x.min(right), y, right.saturating_sub(x.min(right)), 1);
    Some(PromptTokens {
        source,
        kind,
        query,
    })
}

/// chip 宽度 = ` {label} `(左右各 1 空格 padding;label 含字形图标)。
fn chip_width(label: &str) -> u16 {
    u16::try_from(label.width().saturating_add(2)).unwrap_or(u16::MAX)
}

/// 画一枚填充背景的 chip:` {label} `,`label`(含字形图标)给定色加粗、`bg` 填充底。
/// 身份靠图标 + 颜色,无 sigil。
fn draw_chip(frame: &mut Frame<'_>, rect: Rect, label: &str, label_fg: Color, bg: Color) {
    if rect.width == 0 {
        return;
    }
    let fill = Style::new().bg(bg);
    let spans = vec![
        Span::styled(" ", fill),
        Span::styled(
            label.to_owned(),
            fill.fg(label_fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", fill),
    ];
    frame.render_widget(Paragraph::new(Line::from(spans)), rect);
}

/// 把 source 色混向 `base`(80% base + 20% source 色)得极暗调染色,作 source chip 底——
/// 有 source 色身份感又不抢图标。非 RGB 色(理论上不出现:主题色恒 RGB)退回 `surface0`。
fn dark_tint(c: Color, theme: &Theme) -> Color {
    match (c, theme.base) {
        (Color::Rgb(r, g, b), Color::Rgb(br, bg, bb)) => {
            let mix = |src: u8, base: u8| -> u8 {
                u8::try_from((u16::from(base) * 4 + u16::from(src)) / 5).unwrap_or(base)
            };
            Color::Rgb(mix(r, br), mix(g, bg), mix(b, bb))
        }
        _ => theme.surface0,
    }
}

/// 画 query 输入区:`show_cursor` 时以文本光标为界、反色罩住光标处字符(光标可落词中),
/// 否则只画文本(焦点不在 query 段时不显光标)。
fn draw_query(
    frame: &mut Frame<'_>,
    rect: Rect,
    session: &SearchSession,
    theme: &Theme,
    show_cursor: bool,
) {
    if rect.width == 0 {
        return;
    }
    let text = Style::new().fg(theme.text);
    let (before, after) = session.query_split();
    let spans = if show_cursor {
        cursor_spans(before.to_owned(), after, text)
    } else {
        vec![Span::styled(format!("{before}{after}"), text)]
    };
    frame.render_widget(Paragraph::new(Line::from(spans)), rect);
}

/// 画 source / kind chip 的展开下拉(下拉归属某 chip 段且动画未归零时)。复用 PopMenu +
/// [`render_overlay`] 的平滑揭开(脱 overlay 栈),贴对应 chip 左下、自 prompt 框下沿垂下,
/// 盖在 results 面板之上(调用方在面板之后画);高亮行 = `seg_sel`。
///
/// # Params:
///   - `prompt_area`: prompt 外框矩形(与 [`draw_prompt`] 同一 `area`,据此算 chip 子矩形)
///   - `state`: 应用态(读 channel 搜索子域 + caps;`render_overlay` 取 cfg 的揭开风格)
pub(crate) fn draw_prompt_dropdown(
    frame: &mut Frame<'_>,
    prompt_area: Rect,
    state: &AppState,
    theme: &Theme,
) {
    let rs = &state.channel_search;
    // 画下拉归属的 chip 段(与 focus 解耦):切到 query / 别的 chip 后,仍把上一个 chip 的
    // 收起动画画完。
    let Some(seg) = rs.reveal_seg() else {
        return;
    };
    // 动画态判渲染:收起后视觉收尾期继续画着往回收,播完归零才停。
    if !rs.dropdown_active() {
        return;
    }
    let Some(tokens) = prompt_tokens(prompt_area, rs) else {
        return;
    };
    let (title, chip, items): (&str, Rect, Vec<MenuItem>) = match seg {
        PromptSegment::Source => {
            let Some(rect) = tokens.source else {
                return;
            };
            // 行 fg = 各源徽标色(身份靠图标 + 颜色,与 chip 一致);kind 下拉保持中性。
            let items = rs
                .source_options(&state.caps)
                .iter()
                .map(|s| {
                    MenuItem::display_tinted(
                        s.label(),
                        resolve_source_color(theme, state.cfg.sources(), *s),
                    )
                })
                .collect();
            ("source", rect, items)
        }
        PromptSegment::Kind => {
            let items = rs
                .kind_options(&state.caps)
                .iter()
                .map(|k| MenuItem::display(k.label()))
                .collect();
            ("kind", tokens.kind, items)
        }
        PromptSegment::Query => return,
    };
    if items.is_empty() {
        return;
    }
    // 锚点贴 chip 左缘、置于 prompt 框底边行:`Placement::Below` 则下拉落在框**下方**,
    // 不吃掉 prompt 的底边框。
    let anchor = Rect::new(
        chip.x,
        prompt_area.bottom().saturating_sub(1),
        chip.width,
        1,
    );
    // 复用 PopMenu 的锚定渲染 + render_overlay 的平滑揭开(脱 overlay 栈:display-only 菜单 +
    // 自带的 seg_reveal 进度当 scale,键交互仍走 inline)。强制 Left 对齐贴 chip 左下展开。
    let menu = PopMenu::display(title, items, anchor, Placement::Below, rs.seg_sel());
    render_overlay(
        frame,
        frame.area(),
        &menu,
        rs.seg_reveal(),
        /*focused*/ true,
        state,
        theme,
    );
}

/// 画结果列:bordered `results` 外框 + 结果行(当前光标行高亮)。
///
/// 光标行高亮分两档:焦点在结果列时 accent 亮高亮;否则(在 prompt / detail)走暗调高亮,
/// 仍标出"回得去"的光标位置而不抢视觉。
///
/// # Params:
///   - `state`: 应用态(读 channel 搜索子域 + scrolloff / 缓动拍数 / morph 进度)
///   - `border_focused`: 边框是否高亮(焦点环滑动期由调用方置 `false`)
pub fn draw_results(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    border_focused: bool,
) {
    let rs = &state.channel_search;
    let mut block = Block::new()
        .borders(Borders::ALL)
        .border_style(border_style(border_focused, theme))
        .border_type(BorderType::Rounded)
        .title("results");
    // 左下角位置标:lazy 分页未榨干显 `n / total+`(至少 total、可能更多),短页确认榨干才去 `+`。
    if let Some(kr) = rs.active_results().filter(|kr| kr.len() != 0) {
        block = block.title_bottom(
            Line::from(result_position_label(kr.sel(), kr.len(), kr.exhausted))
                .style(Style::new().fg(theme.overlay)),
        );
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if rs.source.is_none() {
        return;
    }
    // 无结果可列时画居中 lite 提示,三态分流(不与「占首行的可高亮列表项」混):
    //   首页在飞 → 旋转 spinner「searching」;到货 0 条(bucket 在但空) → 「no results」;
    //   尚未搜索(无 bucket、不在飞) → 「type a query」。
    let Some(kr) = rs.active_results().filter(|kr| kr.len() != 0) else {
        if rs.current_loading() {
            let glyph = spinner::glyph(
                state.cfg.tui().animation().spinner_frames(),
                rs.spinner_counter(),
            );
            draw_centered_hint(frame, inner, &format!("{glyph} searching"), theme);
        } else if rs.active_results().is_some() {
            draw_centered_hint(frame, inner, "no results", theme);
        } else {
            draw_centered_hint(frame, inner, "type a query", theme);
        }
        return;
    };
    let (header, rows, widths) = result_table(
        &kr.results,
        kr.list().sel(),
        // 表格选中行的 fade 实际会被 row_highlight_style 整行 fg 盖掉(刻意保留整行
        // accent,见 MarqueeCtx::fade_to 注);fade_to 仍按其底色给,不误导插值方向。
        &MarqueeCtx::new(state, theme, /*fade_to*/ theme.surface0),
        inner.width,
        theme,
    );
    // 整行底色高亮(对齐 tracks/playlist/queue 的 row_highlight):bg 铺满整行,非仅文字变色。
    let highlight = highlight_style(
        theme,
        rs.focus_permille(
            *state.cfg.tui().animation().search_focus_transition(),
            SearchFocus::Results,
        ),
    );
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD)))
        .row_highlight_style(highlight)
        .highlight_symbol("▌ ");
    // 视口行数 = 内区高 - 表头(边框归 block);offset 跨帧持久 + 缓动平移。
    // 退场 morph 中面板是收缩瞬态(只读展示,不得用瞬态 viewport 改写滚动目标),仅稳态(at_max)推进。
    let viewport = usize::from(inner.height.saturating_sub(1));
    let motion = if rs.active.at_max() {
        ScrollMotion::Advancing {
            scrolloff: state.scrolloff(),
            glide_ticks: state.list_glide_ticks(),
        }
    } else {
        ScrollMotion::Frozen
    };
    render_scroll_table(
        frame.buffer_mut(),
        inner,
        table,
        kr.list(),
        kr.len(),
        viewport,
        motion,
    );
}

/// 结果列底标 ` n / total[+] `(1-based 当前位 / 已加载条数)。
///
/// 结果是 lazy 分页累积:`exhausted`(短页/空页确认榨干)才省 `+`,`total` 即全部;否则缀 `+`
/// 表示「至少 total、可能还有下一页」。调用方已保证 `total != 0`。
///
/// # Params:
///   - `sel`: 0-based 选中行
///   - `total`: 已加载结果条数
///   - `exhausted`: 是否已确认搜完(无更多页)
fn result_position_label(sel: usize, total: usize, exhausted: bool) -> String {
    let more = if exhausted { "" } else { "+" };
    format!(" {} / {total}{more} ", sel.saturating_add(1).min(total))
}

/// 空结果列的居中 lite 提示(暗调斜体,水平 + 垂直居中);非可高亮列表行。
fn draw_centered_hint(frame: &mut Frame<'_>, inner: Rect, text: &str, theme: &Theme) {
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let strip = Rect::new(inner.x, inner.y + inner.height / 2, inner.width, 1);
    let hint = Line::from(text.to_owned()).style(
        Style::new()
            .fg(theme.overlay)
            .add_modifier(Modifier::ITALIC),
    );
    frame.render_widget(Paragraph::new(hint).alignment(Alignment::Center), strip);
}

/// 把一页结果载荷按类型转成「表头 + 列对齐表格行 + 列宽约束」(调用方已保证非空)。
///
/// 每类型一套列与表头:主名 Fill + 类型特有的次/计量列。一个 payload 只含单一实体类型,故按
/// 类型选一套;主名走 `text`、次列 `subtext`、计量列 `overlay`,层级与 library 表一致。计量
/// 列为裸数字,含义由表头说明(同 library 约定),省去逐行重复单位词。
fn result_table(
    payload: &SearchPayload,
    sel: usize,
    marquee: &MarqueeCtx<'_>,
    inner_w: u16,
    theme: &Theme,
) -> (Vec<Cell<'static>>, Vec<Row<'static>>, Vec<Constraint>) {
    let main = Style::new().fg(theme.text);
    let sub = Style::new().fg(theme.subtext);
    let meta = Style::new().fg(theme.overlay);
    match payload {
        // 歌曲:标题 · 艺人 · 时长。
        SearchPayload::Songs(songs) => {
            let widths = vec![
                Constraint::Fill(3),
                Constraint::Fill(2),
                Constraint::Length(5),
            ];
            // highlight_symbol "▌ " 恒占 2 列;title 是第 0 列。
            let title_w = resolve_column_widths(inner_w, &widths, 2)
                .first()
                .copied()
                .unwrap_or(0);
            let rows = songs
                .iter()
                .enumerate()
                .map(|(idx, s)| {
                    let mut title_spans = vec![Span::styled(s.name.clone(), main)];
                    title_spans.extend(alias_span(s.alias.as_deref(), theme));
                    let title_cell = if idx == sel {
                        Cell::from(marquee.line(
                            title_spans,
                            Slot::SearchResults,
                            &s.id.qualified(),
                            title_w,
                        ))
                    } else {
                        Cell::from(Line::from(title_spans))
                    };
                    let row = Row::new(vec![
                        title_cell,
                        Cell::from(Span::styled(join_artists(&s.artists), sub)),
                        Cell::from(Span::styled(format_ms_opt(s.duration_ms), meta)),
                    ]);
                    if s.unavailable {
                        row.style(theme.unavailable_row())
                    } else {
                        row
                    }
                })
                .collect();
            (
                vec![Cell::from("title"), Cell::from("artist"), Cell::from("len")],
                rows,
                widths,
            )
        }
        // 专辑:专辑名 · 艺人 · 曲目数(表头标 tracks)。列表投影拿不到曲目数时画 `-`(未知,非
        // 0 空专辑);下钻 album_detail 回填真值(见 KindResults::fill_album_detail)。
        SearchPayload::Albums(albums) => {
            let rows = albums
                .iter()
                .map(|a| {
                    let tracks = a
                        .track_count
                        .map_or_else(|| "-".to_owned(), |n| n.to_string());
                    Row::new(vec![
                        Cell::from(Span::styled(a.name.clone(), main)),
                        Cell::from(Span::styled(join_artists(&a.artists), sub)),
                        Cell::from(Span::styled(tracks, meta)),
                    ])
                })
                .collect();
            (
                vec![
                    Cell::from("album"),
                    Cell::from("artist"),
                    Cell::from("tracks"),
                ],
                rows,
                vec![
                    Constraint::Fill(3),
                    Constraint::Fill(2),
                    Constraint::Length(6),
                ],
            )
        }
        // 歌单:歌单名 · 曲目数(裸数字,表头标 tracks)。
        SearchPayload::Playlists(playlists) => {
            let rows = playlists
                .iter()
                .map(|p| {
                    Row::new(vec![
                        Cell::from(Span::styled(p.name.clone(), main)),
                        Cell::from(Span::styled(p.track_count.to_string(), meta)),
                    ])
                })
                .collect();
            (
                vec![Cell::from("playlist"), Cell::from("tracks")],
                rows,
                vec![Constraint::Fill(1), Constraint::Length(6)],
            )
        }
        // 歌手:歌手名 · 关注数(裸缩写,表头标 fans)。
        SearchPayload::Artists(artists) => {
            let rows = artists
                .iter()
                .map(|a| {
                    Row::new(vec![
                        Cell::from(Span::styled(a.name.clone(), main)),
                        Cell::from(Span::styled(
                            a.follower_count
                                .map_or_else(|| "-".to_owned(), humanize_count),
                            meta,
                        )),
                    ])
                })
                .collect();
            (
                vec![Cell::from("artist"), Cell::from("fans")],
                rows,
                vec![Constraint::Fill(1), Constraint::Length(6)],
            )
        }
    }
}

/// 多艺人名 join 成 `艺人1, 艺人2`(无艺人为空串)。
pub(super) fn join_artists(artists: &[ArtistRef]) -> String {
    artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<&str>>()
        .join(", ")
}

/// 大计数缩写:< 1 万原样,≥ 1 万记 `Nk`,≥ 100 万记 `NM`(关注数列窄,纯整数无浮点)。
pub(super) fn humanize_count(n: u64) -> String {
    match n {
        0..=9_999 => n.to_string(),
        10_000..=999_999 => format!("{}k", n / 1000),
        _ => format!("{}M", n / 1_000_000),
    }
}

#[cfg(test)]
mod tests {
    use mineral_channel_core::ChannelCaps;
    use mineral_model::{SearchKind, SourceKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use rustc_hash::FxHashMap;

    use crate::components::layout::shared::marquee::MarqueeCtx;
    use crate::render::theme::{Theme, resolve_source_color};
    use crate::runtime::marquee::{Marquees, Slot};
    use crate::runtime::state::{PromptSegment, SearchPage};

    /// 测试用 marquee 上下文(gap 取默认配置同款,fade 关)。
    fn marquee_ctx(m: &Marquees) -> MarqueeCtx<'_> {
        MarqueeCtx {
            marquees: m,
            gap: "  ✦  ",
            gap_style: ratatui::style::Style::new(),
            fade_to: ratatui::style::Color::Reset,
            fade_cols: 3,
        }
    }

    use super::{
        chip_width, draw_prompt, draw_prompt_dropdown, prompt_tokens, result_position_label,
    };

    /// 选中的歌曲结果行:title 溢出按 marquee 相位滚动,推进拍数后从对应列起显示。
    #[test]
    fn song_result_selected_row_marquees() -> color_eyre::Result<()> {
        use mineral_task::SearchPayload;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::widgets::Table;

        use crate::test_support::{song, with_name};

        let theme = Theme::default();
        let payload = SearchPayload::Songs(vec![with_name(
            song("1"),
            "abcdefghijklmnopqrstuvwxyz0123456789",
        )]);
        let mut mq = Marquees::test_loop(/*step_ticks*/ 1, /*pause_ticks*/ 0);
        let render = |mq: &Marquees| -> color_eyre::Result<String> {
            let (_header, rows, widths) = super::result_table(
                &payload,
                /*sel*/ 0,
                &marquee_ctx(mq),
                /*inner_w*/ 40,
                &theme,
            );
            let mut t = Terminal::new(TestBackend::new(40, 1))?;
            t.draw(|f| f.render_widget(Table::new(rows, widths), f.area()))?;
            let buf = t.backend().buffer();
            Ok((0..buf.area.width)
                .filter_map(|x| buf.cell((x, 0)).map(ratatui::buffer::Cell::symbol))
                .collect::<String>())
        };
        assert!(render(&mq)?.starts_with("abcdef"), "建档帧应从歌名开头显示");
        for _ in 0..4 {
            mq.tick();
        }
        let scrolled = render(&mq)?;
        assert!(
            scrolled.starts_with("efghij"),
            "推进 4 拍后应从第 5 字符起: {scrolled}"
        );
        let _ = Slot::SearchResults; // 槽由 result_table 内部选,此测试无需直接引用
        Ok(())
    }

    /// 歌曲结果行:带别名的歌名后缀暗色 ` (alias)`(overlay),无别名行无后缀。
    #[test]
    fn song_result_row_appends_dim_alias() -> color_eyre::Result<()> {
        use mineral_task::SearchPayload;
        use ratatui::widgets::Table;

        let theme = Theme::default();
        // 真实译名样本:迷星叫 / 叫喊迷星;第二行无别名对照(分隔符 / 括号由 alias_span 单测锁定)。
        let payload = SearchPayload::Songs(vec![
            mineral_test::aliased_song(),
            mineral_test::song("Plain"),
        ]);
        let still = Marquees::test_loop(/*step_ticks*/ 1, /*pause_ticks*/ u32::MAX);
        let (_header, rows, widths) = super::result_table(
            &payload,
            /*sel*/ 0,
            &marquee_ctx(&still),
            /*inner_w*/ 50,
            &theme,
        );
        let mut t = Terminal::new(TestBackend::new(50, 2))?;
        t.draw(|f| f.render_widget(Table::new(rows, widths), f.area()))?;
        let buf = t.backend().buffer();
        let line = |y: u16| -> String {
            (0..buf.area.width)
                .filter_map(|x| buf.cell((x, y)).map(ratatui::buffer::Cell::symbol))
                .collect::<String>()
        };
        // 别名内容:去 CJK 补位 cell 的空格后验存在。
        assert!(
            line(0).replace(' ', "").contains("迷星叫(Mayoiuta)"),
            "带别名行应有别名内容: {}",
            line(0)
        );
        assert!(!line(1).contains('('), "无别名行不应有后缀: {}", line(1));
        let alias_fg = (0..buf.area.width)
            .find_map(|x| buf.cell((x, 0)).filter(|c| c.symbol() == "(").map(|c| c.fg));
        assert_eq!(alias_fg, Some(theme.overlay), "别名后缀应为 overlay 暗色");
        Ok(())
    }

    /// 结果列底标:lazy 分页未榨干缀 `+`(还有下一页可能),短页确认榨干去 `+`(total 即全部);
    /// 1-based 当前位、越界钳到 total。
    #[test]
    fn result_position_label_marks_pagination() {
        assert_eq!(
            result_position_label(/*sel*/ 0, /*total*/ 20, /*exhausted*/ false),
            " 1 / 20+ ",
            "未榨干:可能还有下一页 → +"
        );
        assert_eq!(
            result_position_label(0, 20, /*exhausted*/ true),
            " 1 / 20 ",
            "短页确认榨干 → 无 +"
        );
        assert_eq!(
            result_position_label(/*sel*/ 4, 20, true),
            " 5 / 20 ",
            "1-based 当前位"
        );
        assert_eq!(
            result_position_label(/*sel*/ 99, 20, false),
            " 20 / 20+ ",
            "越界钳到 total"
        );
    }

    /// NETEASE 单源 caps(searchable = 给定 kinds)。
    fn caps(kinds: Vec<SearchKind>) -> FxHashMap<SourceKind, ChannelCaps> {
        let mut m = FxHashMap::default();
        m.insert(
            SourceKind::NETEASE,
            ChannelCaps::builder()
                .searchable(kinds)
                .playlist_edit(false)
                .artist_sections(mineral_channel_core::ArtistSections::new(vec![
                    mineral_channel_core::ArtistSectionKind::TopSongs,
                    mineral_channel_core::ArtistSectionKind::Albums,
                ]))
                .build(),
        );
        m
    }

    /// 进入 search 并落到 NETEASE,得到带当前会话的状态。
    fn entered(kinds: Vec<SearchKind>) -> SearchPage {
        let mut rs = SearchPage::new(/*layout_ticks*/ 1, /*ring_ticks*/ 1);
        rs.enter(&caps(kinds));
        rs
    }

    /// prompt_tokens:source chip 贴 inner 左、kind chip 紧随(各空 1 列)、query 区接其右。
    #[test]
    fn prompt_tokens_lays_out_chips_left_to_right() -> color_eyre::Result<()> {
        let rs = entered(vec![SearchKind::Song]);
        let area = Rect::new(0, 0, 40, 3);
        let t =
            prompt_tokens(area, &rs).ok_or_else(|| color_eyre::eyre::eyre!("有会话应得 tokens"))?;
        let src_w = chip_width(SourceKind::NETEASE.label());
        let src = t
            .source
            .ok_or_else(|| color_eyre::eyre::eyre!("有 source 应得 chip"))?;
        assert_eq!(src, Rect::new(1, 1, src_w, 1), "source chip 贴 inner 左上");
        assert_eq!(
            t.kind.x,
            1 + src_w + 1,
            "kind chip 紧随 source chip + 1 列空隙"
        );
        assert_eq!(t.kind.y, 1, "同一内容行");
        assert_eq!(
            t.query.x,
            t.kind.x + t.kind.width + 1,
            "query 区接 kind chip 右 + 1 列空隙"
        );
        Ok(())
    }

    /// draw_prompt 快照:填充背景的 source / kind chip(去 sigil、含图标)+ 光标落词中(te|st)。
    #[test]
    fn prompt_chips_and_cursor_snapshot() -> color_eyre::Result<()> {
        let mut rs = entered(vec![SearchKind::Song]);
        if let Some(s) = rs.current_mut() {
            for c in "test".chars() {
                s.push_query_char(c);
            }
            s.cursor_left();
            s.cursor_left();
        }
        let cfg = mineral_config::Config::defaults()?;
        let mut terminal = Terminal::new(TestBackend::new(40, 3))?;
        terminal.draw(|f| {
            draw_prompt(
                f,
                f.area(),
                &rs,
                &Theme::default(),
                cfg.sources(),
                /*border_focused*/ true,
            )
        })?;
        crate::test_support::assert_snap!(
            "token prompt:source/kind chip(去 sigil 含图标)+ 光标落 te|st 词中",
            terminal.backend()
        );
        Ok(())
    }

    /// chip 下拉快照:focus 在 kind chip 段 + 展开 settle → prompt 框下方垂下候选,高亮当前行。
    /// 复用 PopMenu + render_overlay 渲染(脱栈),故需真 `AppState`(取 cfg 的揭开风格/对齐)。
    #[test]
    fn kind_dropdown_open_snapshot() -> color_eyre::Result<()> {
        let (mut app, _submitted) = crate::test_support::app_with_channel_search_probed(vec![
            SearchKind::Song,
            SearchKind::Album,
            SearchKind::Artist,
        ])?;
        // 焦点落 kind chip 段、下拉展开(高亮当前 Song = idx 0)。
        app.state
            .channel_search
            .set_prompt_seg(PromptSegment::Kind, 0);
        // 推进展开动画到 settle(scale 满值,render_overlay 走完全展开分支,截到稳态框)。
        for _ in 0..64 {
            app.state.channel_search.tick();
        }
        let prompt = Rect::new(0, 0, 40, 3);
        let mut terminal = Terminal::new(TestBackend::new(40, 10))?;
        terminal.draw(|f| {
            draw_prompt(
                f,
                prompt,
                &app.state.channel_search,
                &app.theme,
                app.state.cfg.sources(),
                /*border_focused*/ false,
            );
            draw_prompt_dropdown(f, prompt, &app.state, &app.theme);
        })?;
        crate::test_support::assert_snap!(
            "kind chip 下拉展开:prompt 下方候选按 default.lua 全局 kind 序,高亮当前 songs",
            terminal.backend()
        );
        Ok(())
    }

    /// source 下拉行按各自 source 徽标色着色(身份靠图标 + 颜色,与 chip 一致);
    /// 选中行(加底色)也不夺 fg。
    #[test]
    fn source_dropdown_rows_tinted_by_source_color() -> color_eyre::Result<()> {
        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        app.state.caps.insert(
            SourceKind::BILIBILI,
            ChannelCaps::builder()
                .searchable(vec![SearchKind::Song])
                .playlist_edit(false)
                .artist_sections(mineral_channel_core::ArtistSections::new(vec![
                    mineral_channel_core::ArtistSectionKind::Albums,
                ]))
                .build(),
        );
        app.state
            .channel_search
            .set_prompt_seg(PromptSegment::Source, 0);
        for _ in 0..64 {
            app.state.channel_search.tick();
        }
        let prompt = Rect::new(0, 0, 40, 3);
        let mut terminal = Terminal::new(TestBackend::new(40, 10))?;
        terminal.draw(|f| {
            draw_prompt(
                f,
                prompt,
                &app.state.channel_search,
                &app.theme,
                app.state.cfg.sources(),
                /*border_focused*/ false,
            );
            draw_prompt_dropdown(f, prompt, &app.state, &app.theme);
        })?;
        let buffer = terminal.backend().buffer();
        // 下拉在 prompt 框(前 3 行)之下;按 source 图标字形定位行,跳过 chip 本体的同字形格。
        let foreground_of = |glyph: &str| {
            (3..buffer.area.height).find_map(|y| {
                (0..buffer.area.width).find_map(|x| {
                    let cell = buffer.cell((x, y))?;
                    if cell.symbol() == glyph {
                        cell.style().fg
                    } else {
                        None
                    }
                })
            })
        };
        let netease =
            resolve_source_color(&app.theme, app.state.cfg.sources(), SourceKind::NETEASE);
        let bilibili =
            resolve_source_color(&app.theme, app.state.cfg.sources(), SourceKind::BILIBILI);
        assert_eq!(
            foreground_of("♫"),
            Some(netease),
            "netease 行前景 = 其徽标色"
        );
        assert_eq!(
            foreground_of("▶"),
            Some(bilibili),
            "bilibili 行(选中,带底色)前景仍 = 其徽标色"
        );
        Ok(())
    }
}
