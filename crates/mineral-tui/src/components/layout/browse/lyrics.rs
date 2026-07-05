//! Lyrics 面板:按 [`crate::runtime::state::AppState::current_lines`] 渲染当前行 + 邻近行,
//! 当前行高亮居中,上下各若干行 dim。无歌词时 fallback "♪ no lyrics"。
//!
//! 有逐字歌词时,中心行走字级 wipe 渲染:已唱的字 = `theme.text` + Bold,
//! 未唱的字 = `theme.overlay` dim。邻行无论是否有逐字都按整行 dim 渲染。
//!
//! `t` 键打开副歌词(翻译 / 罗马音)后,每个可见原文行下方紧跟一条静态副行;
//! 副行不参与 wipe,恒按 muted 样式渲染。

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use mineral_model::{LyricLine, Word};

use crate::render::anim::ease_in_out;
use crate::render::color::lerp_color;
use crate::render::theme::Theme;
use crate::runtime::playback::SyncTrust;
use crate::runtime::state::{AppState, LyricExtra};

/// 渲染 lyrics 面板到给定 [`Rect`]。
///
/// # Params:
///   - `motion`: 呈现模式。[`LyricMode::Compact`] 给嵌入面板(紧凑 + 瞬时高亮);
///     [`LyricMode::Immersive`] 给全屏(行间距 + 缓动平移 + 高亮交叉淡入)。
pub fn draw(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme, motion: LyricMode) {
    let lines = state.current_lines().filter(|v| !v.is_empty());
    let extra = state.active_lyric_extra();
    let trust = state.playback.sync_trust();

    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(title_left_spans(
            lines.is_some_and(mineral_model::has_words),
            lines.is_some_and(mineral_model::has_timed),
            trust,
            theme,
        )))
        .title_top(
            Line::from(title_right_spans(state.has_extra_lyrics(), extra, theme)).right_aligned(),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // 单一行序列:逐字行字级 wipe、其余整行;有时间戳行驱动定位,无时间戳行静态参与渐暗。
    let Some(lines) = lines else {
        draw_fallback(frame, inner, theme);
        return;
    };
    let position_ms = state.playback.position_ms;
    // 时间轴失真(顶换流时长对不上)→ 不认当前行:无高亮、无自动跟随,退成静态整篇
    //(手动滚动照常),与「无时间戳歌词」走同一条渲染路径。
    let cur = match trust {
        SyncTrust::Broken => None,
        SyncTrust::Native | SyncTrust::Borrowed => mineral_model::current_line(lines, position_ms),
    };
    paint_window(
        frame,
        inner,
        WindowInput {
            lines,
            cur,
            position_ms,
            extra,
            motion,
            // 行间距:脚本覆盖优先,无覆盖回落配置值。
            fullscreen_line_gap: state
                .ui_overrides
                .fullscreen_line_gap
                .unwrap_or(*state.cfg.tui().lyrics().fullscreen_line_gap()),
            compact_line_gap: state
                .ui_overrides
                .compact_line_gap
                .unwrap_or(*state.cfg.tui().lyrics().compact_line_gap()),
            scroll_ms: *state.cfg.tui().lyrics().scroll_ms(),
            // 手动滚动是全屏沉浸态专属;紧凑面板恒附着,不继承脱离偏移。
            manual_anchor_milli: match motion {
                LyricMode::Immersive => state.manual_lyric_anchor_milli(),
                LyricMode::Compact => None,
            },
            manual_focus: match motion {
                LyricMode::Immersive => state.manual_lyric_focus_line(),
                LyricMode::Compact => None,
            },
        },
        theme,
    );
}

/// 左上标识:数据档(`lyrics` / `synced` / `synced ✦`)× 时间轴信任档。两档同步用
/// 不同高亮区分——行级 `synced` 用 accent_2(sapphire),逐字 `synced ✦` 用 accent
/// (mauve);`lyrics · ` 前缀恒 subtext 弱化。顶换流:[`SyncTrust::Borrowed`] 在
/// synced 后缀 `~`(yellow,「可能漂移」);[`SyncTrust::Broken`] 整档换成
/// `unsynced`(yellow,同步已放弃)。
///
/// # Params:
///   - `has_words`: 是否有逐字歌词
///   - `has_lrc`: 是否有行级 LRC
///   - `trust`: 时间轴信任档(无 LRC 时无同步可言,不参与)
///   - `theme`: 取色
///
/// # Return:
///   组成 ` lyrics · synced ✦ ` 的分色 Span 序列(首尾留空格)。
fn title_left_spans(
    has_words: bool,
    has_lrc: bool,
    trust: SyncTrust,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let base = Style::new().fg(theme.subtext);
    let base_lyrics = Span::styled(" lyrics · ", base);
    let mark = |color| {
        Style::new()
            .fg(color)
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::ITALIC)
    };

    if !has_lrc {
        return vec![Span::styled(" lyrics ", base)];
    }
    if trust == SyncTrust::Broken {
        return vec![
            base_lyrics,
            Span::styled("unsynced", mark(theme.yellow)),
            Span::styled(" ", base),
        ];
    }
    let mut spans = if has_words {
        vec![base_lyrics, Span::styled("synced ✦", mark(theme.accent))]
    } else {
        vec![base_lyrics, Span::styled("synced", mark(theme.accent_2))]
    };
    if trust == SyncTrust::Borrowed {
        spans.push(Span::styled(" ~", mark(theme.yellow)));
    }
    spans.push(Span::styled(" ", base));
    spans
}

/// 右上提示:当前生效的副歌词档 + `[t]` 按键提示(方括号示意这是个按键)。翻译标 `tr`
/// (green)、罗马音标 `ro`(peach),均 bold + italic;`[t]` 及分隔点用 overlay 弱化。
/// 没有任何副歌词可切换时返回空序列(不显示提示)。
///
/// # Params:
///   - `has_extra`: 是否有任一副歌词(翻译 / 罗马音)可切换
///   - `extra`: 当前生效(且非空)的副歌词档;`None` 只显示按键
///   - `theme`: 取色
///
/// # Return:
///   组成 ` tr · [t] ` / ` [t] ` 的分色 Span 序列;无副歌词时为空。
fn title_right_spans(
    has_extra: bool,
    extra: Option<LyricExtra>,
    theme: &Theme,
) -> Vec<Span<'static>> {
    if !has_extra {
        return Vec::new();
    }
    let key = Style::new().fg(theme.overlay);
    let mark = |color| {
        Style::new()
            .fg(color)
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::ITALIC)
    };
    let mut spans = vec![Span::styled(" ", key)];
    match extra {
        Some(LyricExtra::Translation) => {
            spans.push(Span::styled("tr", mark(theme.green)));
            spans.push(Span::styled(" · ", key));
        }
        Some(LyricExtra::Romanization) => {
            spans.push(Span::styled("ro", mark(theme.peach)));
            spans.push(Span::styled(" · ", key));
        }
        Some(LyricExtra::None) | None => {}
    }
    spans.push(Span::styled("[t]", key));
    spans.push(Span::styled(" ", key));
    spans
}

/// 一个视觉行:原文行(`Primary`)、其下方的副歌词(`Secondary`),或行间空行(`Spacer`)。
enum Cell {
    /// 原文行,引用 `lines` 中的索引。
    Primary {
        /// 在 `lines` 中的行索引。
        line_idx: usize,
    },

    /// 副歌词行(翻译 / 罗马音),自带文本。
    Secondary {
        /// 副歌词文本。
        text: String,
    },

    /// 行间空行(仅 [`LyricMode::Immersive`] 垫入),渲染为空,只占位拉开行距。
    Spacer,
}

/// 没歌词时居中渲染一行 `♪ no lyrics`(灰色 + 斜体)。
fn draw_fallback(frame: &mut Frame<'_>, inner: Rect, theme: &Theme) {
    let centered_y = inner.y + inner.height / 2;
    let text_area = Rect::new(inner.x, centered_y, inner.width, 1);
    let line = Line::from("♪ no lyrics").style(
        Style::new()
            .fg(theme.overlay)
            .add_modifier(Modifier::ITALIC),
    );
    frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), text_area);
}

/// 取某行已配对的副歌词文本
fn secondary_text(line: &LyricLine, extra: LyricExtra) -> Option<&str> {
    match extra {
        LyricExtra::None => None,
        LyricExtra::Translation => line.translation.as_deref(),
        LyricExtra::Romanization => line.romanization.as_deref(),
    }
}

/// 歌词呈现模式。决定行间距与高亮过渡——同一个 [`draw`] 给两处调用方复用。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LyricMode {
    /// 普通嵌入面板:行紧贴、当前行瞬时高亮(无滚动 / 无淡入)。
    Compact,

    /// 全屏沉浸:行间垫空行、整列缓动平移、当前行高亮交叉淡入。
    Immersive,
}

/// 进度满值(千分比),与 [`crate::render::anim`] 同范式定点。
const SCROLL_FULL: u16 = 1000;

/// 当前行已激活时长 `elapsed_ms` 占过渡窗口 `window_ms` 的千分比,`elapsed >= window` 饱和到
/// 满值。`window_ms == 0` 视作已满(不除零)。
///
/// # Params:
///   - `elapsed_ms`: `position_ms - 当前行起始时间`
///   - `window_ms`: 过渡窗口时长
///
/// # Return:
///   线性进度千分比,`0..=1000`。
fn scroll_progress(elapsed_ms: u64, window_ms: u64) -> u16 {
    if window_ms == 0 || elapsed_ms >= window_ms {
        return SCROLL_FULL;
    }
    u16::try_from(elapsed_ms.saturating_mul(u64::from(SCROLL_FULL)) / window_ms)
        .unwrap_or(SCROLL_FULL)
}

/// 在上一行 `prev_center`、当前行 `cur_center` 两个视觉行索引间,按**已缓动**进度 `eased`
/// (`0..=1000`)线性插值并四舍五入成整数行号。`cur <= prev`(首行 / 无上一行)恒返回
/// `cur_center`,即不滚动。
///
/// # Params:
///   - `prev_center`: 上一行的视觉行索引
///   - `cur_center`: 当前行的视觉行索引
///   - `eased`: 已过 ease-in-out 的进度千分比
///
/// # Return:
///   本帧居中锚点应落的整数视觉行索引,落在 `[prev_center, cur_center]`。
fn scroll_anchor(prev_center: usize, cur_center: usize, eased: u16) -> usize {
    if cur_center <= prev_center {
        return cur_center;
    }
    let delta = cur_center - prev_center;
    let Ok(delta) = u64::try_from(delta) else {
        return cur_center;
    };
    // round(prev + delta·eased/1000):分子 +500 实现四舍五入,全程 u64 定点。
    let offset = (delta.saturating_mul(u64::from(eased)) + 500) / u64::from(SCROLL_FULL);
    prev_center.saturating_add(usize::try_from(offset).unwrap_or(0))
}

/// 把手动滚动的 milli-line 锚点(原文行号 × 1000)映射成 `cells` 中的整数行索引。
///
/// 取相邻两原文行的 cell 位置按小数部分线性插值落到整行;终端无亚格滚动,平滑感来自
/// 状态层逐 tick 缓动推进 milli 值,使这个整数锚点在 cells 间逐格前移(行间有空行时
/// 尤为顺滑)。锚点越出内容界(边界过冲)时沿首 / 末段行距向界外线性外推,画出
/// rubber-band 的"滚出去"帧——返回值因此可为负或超过末 cell,渲染循环对界外行渲空。
///
/// # Params:
///   - `primary_cell`: 各原文行 → 其在 `cells` 中的视觉行索引
///   - `milli`: 缓动锚点(milli-line,过冲时越出 `[0, 行数-1]`)
///
/// # Return:
///   居中锚点应落的 `cells` 带符号索引。
fn manual_cell_anchor(primary_cell: &[usize], milli: i64) -> isize {
    let Some(max_line) = primary_cell.len().checked_sub(1) else {
        return 0;
    };
    let max_line = i64::try_from(max_line).unwrap_or(0);
    // 段下标夹到 [0, max-1]:界内即 milli 所在整行;过冲时落在首 / 末段,frac 越出
    // [0, 1000] 即沿该段行距外推。
    let i = milli.div_euclid(1000).clamp(0, (max_line - 1).max(0));
    let frac = milli - i * 1000;
    let cell_at = |idx: i64| {
        usize::try_from(idx)
            .ok()
            .and_then(|u| primary_cell.get(u).copied())
            .and_then(|c| i64::try_from(c).ok())
            .unwrap_or(0)
    };
    let c0 = cell_at(i);
    // 单行歌词无相邻段可量行距,外推步长兜底 1 cell/行。
    let span = (cell_at((i + 1).min(max_line)) - c0).max(1);
    // round(c0 + span·frac/1000):+500 后向下取整 = 四舍五入;div_euclid 保证负 frac
    // 也朝同一方向取整(截断除法会在 0 附近不对称)。
    let offset = (span.saturating_mul(frac) + 500).div_euclid(1000);
    isize::try_from(c0 + offset).unwrap_or(0)
}

/// 渲染一个歌词窗口所需的内容输入(打包以压参数数)。
#[derive(Clone, Copy)]
struct WindowInput<'a> {
    /// 原文行序列。
    lines: &'a [LyricLine],

    /// 当前行在 `lines` 中的索引;`None` = 前奏未进首句 / 全无时间戳。
    cur: Option<usize>,

    /// 当前播放位置(ms),用于逐字 wipe 进度。
    position_ms: u64,

    /// 生效的副歌词档(翻译 / 罗马音);`None` = 不显示副行。
    extra: Option<LyricExtra>,

    /// 呈现模式:决定行间距与高亮过渡。
    motion: LyricMode,

    /// Immersive 模式的行间距(行,配置 `tui.lyrics.fullscreen_line_gap`,可被脚本覆盖)。
    fullscreen_line_gap: usize,

    /// Compact 模式的行间距(行,配置 `tui.lyrics.compact_line_gap`,可被脚本覆盖)。
    compact_line_gap: usize,

    /// 行切换缓动平移时长(ms,配置 `tui.lyrics.scroll_ms`)。
    scroll_ms: u64,

    /// 手动滚动「脱离播放」的缓动锚点(milli-line = 原文行号 × 1000);`None` = 附着态
    /// (居中跟随播放)。仅全屏沉浸态可能为 `Some`,紧凑面板恒 `None`。
    manual_anchor_milli: Option<i64>,

    /// 脱离态锚定的原文行(手动浏览焦点,渲染半程高亮);`None` = 附着态。
    manual_focus: Option<usize>,
}

/// 渲染以 `cur` 为中心、上下展开的歌词窗口。
///
/// 每个原文行展开成一个 `Primary` 视觉行;`extra` 非空时其下紧跟一个 `Secondary` 视觉行。
/// 居中基准是当前原文行的 `Primary`;中心行若有逐字走字级 wipe,否则整行高亮;
/// 其余 `Primary` 按距中心远近 dim,`Secondary` 恒 muted + 斜体。
/// [`LyricMode::Immersive`] 下行间垫空行、整列缓动平移、当前行高亮交叉淡入。
fn paint_window(frame: &mut Frame<'_>, inner: Rect, input: WindowInput<'_>, theme: &Theme) {
    let WindowInput {
        lines,
        cur,
        position_ms,
        extra,
        motion,
        fullscreen_line_gap,
        compact_line_gap,
        scroll_ms,
        manual_anchor_milli,
        manual_focus,
    } = input;
    let gap = match motion {
        LyricMode::Immersive => fullscreen_line_gap,
        LyricMode::Compact => compact_line_gap,
    };
    // 展开成视觉行序列;记当前行(居中基准)、上一行(平移 / 交叉淡入端)及每条原文行
    // 所在视觉行(`primary_cell`,手动滚动把 milli-line 锚点映射回 cell 用)。
    let mut cells = Vec::<Cell>::new();
    let mut primary_cell = Vec::<usize>::with_capacity(lines.len());
    let mut cur_center = 0usize;
    let mut prev_center = 0usize;
    for (idx, line) in lines.iter().enumerate() {
        // 非首行前垫空行,把相邻行视觉行距拉开(Compact 时 gap=0,退化回紧贴)。
        for _ in 0..(if idx > 0 { gap } else { 0 }) {
            cells.push(Cell::Spacer);
        }
        let here = cells.len();
        primary_cell.push(here);
        if Some(idx) == cur {
            cur_center = here;
        }
        if cur.is_some_and(|c| c > 0 && idx == c - 1) {
            prev_center = here;
        }
        cells.push(Cell::Primary { line_idx: idx });
        if let Some(text) = extra.and_then(|e| secondary_text(line, e)) {
            cells.push(Cell::Secondary {
                text: text.to_owned(),
            });
        }
    }
    // 首行 / 前奏(无上一行)→ prev 落回 cur,锚点不滚动。
    if cur.is_none_or(|c| c == 0) {
        prev_center = cur_center;
    }

    // 高亮交叉淡入进度(当前行淡入 / 上一行退场),只由播放驱动 —— 脱离态下播放照常推进,
    // 高亮 / wipe 仍跟随。Compact 恒到位(prog 满、无 prev),退化回瞬时高亮。
    let (eased, prev_active) = match motion {
        LyricMode::Compact => (SCROLL_FULL, None),
        LyricMode::Immersive => {
            let elapsed = cur.map_or(0, |c| {
                position_ms.saturating_sub(lines.get(c).and_then(|l| l.time_ms).unwrap_or(0))
            });
            let eased = ease_in_out(scroll_progress(elapsed, scroll_ms));
            (eased, cur.filter(|&c| c > 0).map(|c| c - 1))
        }
    };
    // 居中锚点:脱离态居中在手动缓动锚点(milli-line,播放不参与;过冲时为界外带符号
    // 索引);附着态跟随播放(Immersive 走逐行时间驱动平移、Compact 瞬时居中)。
    let anchor = match manual_anchor_milli {
        Some(milli) => manual_cell_anchor(&primary_cell, milli),
        None => isize::try_from(match motion {
            LyricMode::Compact => cur_center,
            LyricMode::Immersive => scroll_anchor(prev_center, cur_center, eased),
        })
        .unwrap_or(0),
    };
    let ctx = CellCtx {
        cur,
        prev: prev_active,
        focus: manual_focus,
        eased,
        position_ms,
    };

    let height = usize::from(inner.height);
    let center_row = height / 2;
    // fade 用:从中心向两侧最远距离的较大者(窗口可能不对称)。
    let max_dist = u64::try_from(center_row.max(height.saturating_sub(center_row + 1)))
        .unwrap_or(0)
        .max(1);
    let denom = max_dist.saturating_sub(1).max(1);

    for row in 0..height {
        // 把行号映射到 cells 的 index:row=center_row 对应缓动锚点 anchor。
        let cell_idx_signed =
            isize::try_from(row).unwrap_or(0) - isize::try_from(center_row).unwrap_or(0) + anchor;
        if cell_idx_signed < 0 {
            continue;
        }
        let Ok(cell_idx) = usize::try_from(cell_idx_signed) else {
            continue;
        };
        let Some(cell) = cells.get(cell_idx) else {
            continue;
        };

        let row_u16 = u16::try_from(row).unwrap_or(0);
        let row_area = Rect::new(inner.x, inner.y + row_u16, inner.width, 1);

        let dist_signed =
            isize::try_from(row).unwrap_or(0) - isize::try_from(center_row).unwrap_or(0);
        let dist = u64::try_from(dist_signed.unsigned_abs()).unwrap_or(0);

        let line = render_cell(cell, lines, ctx, dist, denom, theme);
        frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), row_area);
    }
}

/// 渲染一个视觉行所需的高亮上下文(打包以压参数数)。
#[derive(Clone, Copy)]
struct CellCtx {
    /// 当前行 line index;`None` = 前奏未进首句。
    cur: Option<usize>,

    /// 上一行 line index(交叉淡入的退场端);`None` = 无上一行 / Compact 不淡入。
    prev: Option<usize>,

    /// 脱离态锚定行(手动浏览焦点);`None` = 附着态。当前行优先于焦点行。
    focus: Option<usize>,

    /// 已缓动进度千分比:当前行淡入程度,上一行按 `1000 - eased` 退场。
    eased: u16,

    /// 当前播放位置(ms),用于逐字 wipe 进度。
    position_ms: u64,
}

/// 把一个视觉行渲成 [`Line`]:当前行高亮 / wipe,上一行交叉淡出,其余原文行按距中心 dim,
/// 副歌词行恒 muted,空行渲空。
fn render_cell<'a>(
    cell: &'a Cell,
    lines: &'a [LyricLine],
    ctx: CellCtx,
    dist: u64,
    denom: u64,
    theme: &Theme,
) -> Line<'a> {
    match cell {
        Cell::Spacer => Line::default(),
        Cell::Secondary { text } => {
            // 副行永远比原文淡:从 overlay 起 fade 入背景,加斜体作视觉区分。
            let color = lerp_color(theme.overlay, theme.surface0, dist.saturating_sub(1), denom);
            Line::from(text.as_str()).style(Style::new().fg(color).add_modifier(Modifier::ITALIC))
        }
        Cell::Primary { line_idx } => {
            let line = lines.get(*line_idx);
            // 当前行有逐字 → 字级 wipe(自带 subtext→accent 渐变,天然承担"淡入")。
            if Some(*line_idx) == ctx.cur
                && let Some(words) = line.map(|l| l.kind.words()).filter(|w| !w.is_empty())
            {
                return render_word_line(words, ctx.position_ms, theme);
            }
            let text = line.map(|l| l.kind.text().into_owned()).unwrap_or_default();
            // 其余统一按 emphasis 在距离淡色与 accent 间插值:当前行 e=eased 升入 accent、
            // 上一行 e=1000-eased 从 accent 退出、脱离态锚定行恒半程(介于 now-playing
            // 与普通渐暗之间,标记手动浏览焦点)、其它 e=0 即原距离淡色。
            let emphasis = if Some(*line_idx) == ctx.cur {
                ctx.eased
            } else if Some(*line_idx) == ctx.prev {
                SCROLL_FULL.saturating_sub(ctx.eased)
            } else if Some(*line_idx) == ctx.focus {
                SCROLL_FULL / 2
            } else {
                0
            };
            // 距中心越远越淡入背景。远端 endpoint 用 surface0,任何 bg 色下都是淡出。
            let base = lerp_color(theme.subtext, theme.surface0, dist.saturating_sub(1), denom);
            let color = lerp_color(
                base,
                theme.accent,
                u64::from(emphasis),
                u64::from(SCROLL_FULL),
            );
            let mut style = Style::new().fg(color);
            // 过半激活才加粗:加粗在 eased 跨半时从上一行交到当前行,避免切换瞬间闪一下。
            // 恰为半程的焦点行不加粗,与满 accent + Bold 的 now-playing 行拉开层级。
            if emphasis > SCROLL_FULL / 2 {
                style = style.add_modifier(Modifier::BOLD);
            }
            Line::from(text).style(style)
        }
    }
}

/// 按 `position_ms` 把逐字行渲染成字符级渐变 Span 序列(KTV wipe)。
///
/// `Word.text` 对中文是单字、对英文是整词,所以在 `Word` 内再按 `text.chars()` 等分
/// 时间,每个 Unicode 字符独立 lerp 颜色,得到逐字渐变效果。
fn render_word_line<'a>(words: &'a [Word], position_ms: u64, theme: &Theme) -> Line<'a> {
    let mut spans = Vec::<Span<'a>>::new();
    for w in words {
        push_char_spans(&mut spans, w, position_ms, theme);
    }
    Line::from(spans)
}

/// 把一个 `Word` 按字符均分时间,每字符一个 Span,颜色按 char 内进度 lerp。
fn push_char_spans<'a>(out: &mut Vec<Span<'a>>, word: &'a Word, position_ms: u64, theme: &Theme) {
    let n = word.text.chars().count();
    let Ok(n_u64) = u64::try_from(n) else {
        return;
    };
    if n_u64 == 0 {
        return;
    }
    let total_dur = word.dur_ms.max(1);
    let mut byte_cursor = 0usize;
    for (i, ch) in word.text.chars().enumerate() {
        let Ok(i_u64) = u64::try_from(i) else {
            return;
        };
        let char_start = word.start_ms.saturating_add(i_u64 * total_dur / n_u64);
        let char_end = word
            .start_ms
            .saturating_add((i_u64 + 1) * total_dur / n_u64);
        let char_dur = char_end.saturating_sub(char_start).max(1);
        let elapsed = position_ms.saturating_sub(char_start).min(char_dur);
        // wipe 起点用 subtext(跟 d=1 邻居同亮度,中心行未唱部分不再「比周围暗」);
        // 终点 accent 跟 lrc 兜底中心行同色;整行加 BOLD 让最亮部分再亮一点。
        let color = lerp_color(theme.subtext, theme.accent, elapsed, char_dur);

        let next_byte = byte_cursor.saturating_add(ch.len_utf8());
        let slice = word.text.get(byte_cursor..next_byte).unwrap_or_default();
        byte_cursor = next_byte;

        out.push(Span::styled(
            slice,
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use ratatui::text::Span;

    use super::{title_left_spans, title_right_spans};
    use crate::runtime::state::{AppState, LyricExtra};

    /// 拼接 Span 序列的纯文本(忽略样式),给标题文本形状断言用。
    fn text_of(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }
    use crate::render::theme::Theme;

    use super::{manual_cell_anchor, scroll_anchor, scroll_progress};

    /// `scroll_progress`:elapsed/window 定点千分比,`elapsed >= window` 饱和到 1000。
    #[test]
    fn scroll_progress_clamps_and_scales() {
        assert_eq!(scroll_progress(/*elapsed*/ 0, /*window*/ 280), 0, "行起始");
        assert_eq!(scroll_progress(140, 280), 500, "半程");
        assert_eq!(scroll_progress(280, 280), 1000, "窗口末端到顶");
        assert_eq!(scroll_progress(5000, 280), 1000, "远超窗口(seek)→ 饱和吸附");
        assert_eq!(scroll_progress(140, 0), 1000, "window=0 视作已满,不除零");
    }

    /// `scroll_anchor`:两锚点间按已缓动进度四舍五入;端点到位、中点取中、相等不滚。
    #[test]
    fn scroll_anchor_interpolates_and_snaps() {
        assert_eq!(
            scroll_anchor(/*prev*/ 0, /*cur*/ 2, /*eased*/ 0),
            0,
            "起步停上一行"
        );
        assert_eq!(scroll_anchor(0, 2, 1000), 2, "满进度到当前行");
        assert_eq!(scroll_anchor(0, 2, 500), 1, "半程落中间空行(delta=2)");
        assert_eq!(scroll_anchor(0, 3, 500), 2, "delta=3(开副行)半程");
        assert_eq!(
            scroll_anchor(5, 5, 500),
            5,
            "prev==cur(首行/无上一行)不滚动"
        );
    }

    /// `scroll_anchor`:进度从 0→1000 扫一遍,锚点单调不降且全程落在 [prev, cur]。
    #[test]
    fn scroll_anchor_monotonic_within_bounds() {
        let (prev, cur) = (4usize, 6usize);
        let mut last = prev;
        for eased in 0..=1000u16 {
            let a = scroll_anchor(prev, cur, eased);
            assert!(a >= last, "单调不降: eased={eased} a={a} last={last}");
            assert!((prev..=cur).contains(&a), "越界: {a} 不在 [{prev},{cur}]");
            last = a;
        }
        assert_eq!(last, cur, "扫到满进度应抵达当前行");
    }

    /// `manual_cell_anchor`:界内插值不变;越界沿首 / 末段行距外推(rubber-band 过冲帧),
    /// 顶部为负、底部越过末 cell。
    #[test]
    fn manual_cell_anchor_extrapolates_overscroll() {
        let cells = [0usize, 3, 6]; // 3 行,相邻行距 3
        assert_eq!(manual_cell_anchor(&cells, 1000), 3, "界内整行直达");
        assert_eq!(manual_cell_anchor(&cells, 1500), 5, "界内半程插值(round)");
        assert_eq!(
            manual_cell_anchor(&cells, -500),
            -1,
            "顶部过冲:负锚点(首段行距外推)"
        );
        assert_eq!(
            manual_cell_anchor(&cells, 2500),
            8,
            "底部过冲:越过末 cell(末段行距外推)"
        );
        assert_eq!(
            manual_cell_anchor(&[0], -1000),
            -1,
            "单行歌词兜底步长 1 cell/行"
        );
    }

    /// 无当前歌 / 无歌词缓存 → fallback 态。
    #[test]
    fn lyrics_fallback_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        let state = AppState::test_default()?;
        t.draw(|f| {
            super::draw(
                f,
                f.area(),
                &state,
                &Theme::default(),
                super::LyricMode::Compact,
            )
        })?;
        crate::test_support::assert_snap!("歌词:无当前歌 / 无缓存 → fallback", t.backend());
        Ok(())
    }

    /// 逐字 + 翻译档:每个可见原文行下方紧跟翻译副行,标识 `synced ✦ · 译`。
    #[test]
    fn lyrics_words_with_translation_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 14))?;
        let state = crate::test_support::state_with_lyrics(
            LyricExtra::Translation,
            /*with_words*/ true,
        )?;
        t.draw(|f| {
            super::draw(
                f,
                f.area(),
                &state,
                &Theme::default(),
                super::LyricMode::Compact,
            )
        })?;
        crate::test_support::assert_snap!("歌词:逐字 + 翻译副行(所有可见行)", t.backend());
        Ok(())
    }

    /// 脚本覆盖 `lyrics.compact_line_gap = 1`:Compact 模式行间出现空行
    /// (配置默认 0 紧排;覆盖只改渲染态,不碰配置)。
    #[test]
    fn lyrics_compact_gap_override_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 14))?;
        let mut state =
            crate::test_support::state_with_lyrics(LyricExtra::None, /*with_words*/ false)?;
        state.ui_overrides.apply(
            "lyrics.compact_line_gap",
            Some(&mineral_protocol::BusValue::Int(1)),
        );
        t.draw(|f| {
            super::draw(
                f,
                f.area(),
                &state,
                &Theme::default(),
                super::LyricMode::Compact,
            )
        })?;
        crate::test_support::assert_snap!(
            "歌词:脚本覆盖 compact_line_gap=1 → 行间垫空行",
            t.backend()
        );
        Ok(())
    }

    /// 行级(无逐字)+ 罗马音档:标识 `synced · 音`,每行下方紧跟罗马音副行。
    #[test]
    fn lyrics_lrc_with_romanization_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 14))?;
        let state = crate::test_support::state_with_lyrics(
            LyricExtra::Romanization,
            /*with_words*/ false,
        )?;
        t.draw(|f| {
            super::draw(
                f,
                f.area(),
                &state,
                &Theme::default(),
                super::LyricMode::Compact,
            )
        })?;
        crate::test_support::assert_snap!("歌词:行级 + 罗马音副行", t.backend());
        Ok(())
    }

    /// 无翻译 / 无罗马音(《飞鱼转身》)→ 右上不显示 `[t]` 提示,只有左上数据档。
    #[test]
    fn lyrics_no_extra_hides_hint_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 14))?;
        let state = crate::test_support::state_with_lrc_only()?;
        t.draw(|f| {
            super::draw(
                f,
                f.area(),
                &state,
                &Theme::default(),
                super::LyricMode::Compact,
            )
        })?;
        crate::test_support::assert_snap!("歌词:无副歌词 → 右上无 [t] 提示", t.backend());
        Ok(())
    }

    /// 全屏沉浸稳态:当前行落定居中、行间垫空行(对照下方过渡中途帧)。position 62s 落在
    /// line[00:59.31] 起 +2.69s,远超过渡窗口 → 锚点已吸附、无交叉淡入。
    #[test]
    fn lyrics_immersive_steady_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 20))?;
        let state = crate::test_support::state_with_lyrics(
            LyricExtra::Translation,
            /*with_words*/ true,
        )?;
        t.draw(|f| {
            super::draw(
                f,
                f.area(),
                &state,
                &Theme::default(),
                super::LyricMode::Immersive,
            )
        })?;
        crate::test_support::assert_snap!("全屏歌词:稳态(当前行居中 + 行间距)", t.backend());
        Ok(())
    }

    /// 全屏沉浸 + 手动下滚:在自动锚点上叠加偏移,当前行离开正中、窗口整体上移露出后文
    /// (对照上方稳态帧)。synced 歌仍按播放位置算 now-playing 高亮。
    #[test]
    fn lyrics_immersive_manual_scroll_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 20))?;
        let mut state = crate::test_support::state_with_lyrics(
            LyricExtra::Translation,
            /*with_words*/ true,
        )?;
        // 脱离播放、锚定在当前播放行下方 2 行(已 settle):当前行仍在窗内但离开正中。
        state.debug_scroll_lyrics_to_settled(2);
        t.draw(|f| {
            super::draw(
                f,
                f.area(),
                &state,
                &Theme::default(),
                super::LyricMode::Immersive,
            )
        })?;
        crate::test_support::assert_snap!(
            "全屏歌词:脱离播放手动下滚 2 行(居中在锚定行,当前行离开正中)",
            t.backend()
        );
        Ok(())
    }

    /// 全屏沉浸 + 顶部过冲:锚点滚出内容上界(rubber-band 中帧),首行被推到中心下方、
    /// 上方露空白;弹回由状态层驱动,渲染只画当帧锚点。
    #[test]
    fn lyrics_immersive_overscroll_top_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 20))?;
        let mut state = crate::test_support::state_with_lyrics(
            LyricExtra::Translation,
            /*with_words*/ true,
        )?;
        state.debug_scroll_lyrics_to_milli(-1500);
        t.draw(|f| {
            super::draw(
                f,
                f.area(),
                &state,
                &Theme::default(),
                super::LyricMode::Immersive,
            )
        })?;
        crate::test_support::assert_snap!(
            "全屏歌词:顶部过冲帧(首行离开上界,上方露空白)",
            t.backend()
        );
        Ok(())
    }

    /// 全屏沉浸 + 底部过冲:锚点越过末行(rubber-band 中帧),末行升到中心上方、
    /// 下方露空白。
    #[test]
    fn lyrics_immersive_overscroll_bottom_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 20))?;
        let mut state = crate::test_support::state_with_lyrics(
            LyricExtra::Translation,
            /*with_words*/ true,
        )?;
        let max_line = i64::try_from(
            state
                .current_lines()
                .map_or(0, <[mineral_model::LyricLine]>::len)
                .saturating_sub(1),
        )
        .unwrap_or(0);
        state.debug_scroll_lyrics_to_milli(max_line * 1000 + 1500);
        t.draw(|f| {
            super::draw(
                f,
                f.area(),
                &state,
                &Theme::default(),
                super::LyricMode::Immersive,
            )
        })?;
        crate::test_support::assert_snap!(
            "全屏歌词:底部过冲帧(末行越过中心,下方露空白)",
            t.backend()
        );
        Ok(())
    }

    /// 全屏沉浸过渡中途:刚跨入新行(elapsed=120ms < 280ms 窗口)→ 整列缓动平移到一半 +
    /// 当前行高亮交叉淡入。取 position=66_510ms(line[01:06.39] 起 +120ms),开翻译副行使
    /// 相邻行视觉行距=3、中间帧可见。
    #[test]
    fn lyrics_immersive_scroll_midframe_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 20))?;
        let mut state = crate::test_support::state_with_lyrics(
            LyricExtra::Translation,
            /*with_words*/ true,
        )?;
        state.playback.position_ms = 66_510;
        t.draw(|f| {
            super::draw(
                f,
                f.area(),
                &state,
                &Theme::default(),
                super::LyricMode::Immersive,
            )
        })?;
        crate::test_support::assert_snap!(
            "全屏歌词:跨行过渡中途(缓动平移 + 高亮交叉淡入 + 行间距)",
            t.backend()
        );
        Ok(())
    }

    /// 左上数据档三档 × 时间轴信任档(文本形状;颜色不进文本快照)。
    #[test]
    fn title_left_tiers() {
        use super::SyncTrust;
        let th = Theme::default();
        assert_eq!(
            text_of(&title_left_spans(false, false, SyncTrust::Native, &th)),
            " lyrics "
        );
        assert_eq!(
            text_of(&title_left_spans(false, true, SyncTrust::Native, &th)),
            " lyrics · synced "
        );
        assert_eq!(
            text_of(&title_left_spans(true, true, SyncTrust::Native, &th)),
            " lyrics · synced ✦ "
        );
        // 顶换流:Borrowed 后缀 ~;Broken 整档换 unsynced;无 LRC 时信任档不参与。
        assert_eq!(
            text_of(&title_left_spans(false, true, SyncTrust::Borrowed, &th)),
            " lyrics · synced ~ "
        );
        assert_eq!(
            text_of(&title_left_spans(true, true, SyncTrust::Borrowed, &th)),
            " lyrics · synced ✦ ~ "
        );
        assert_eq!(
            text_of(&title_left_spans(true, true, SyncTrust::Broken, &th)),
            " lyrics · unsynced "
        );
        assert_eq!(
            text_of(&title_left_spans(false, false, SyncTrust::Broken, &th)),
            " lyrics "
        );
    }

    /// 顶换流时长差超阈(Broken):放弃逐行同步——窗口锚回篇首、无当前行高亮,
    /// 标识换 `unsynced`(对照同 fixture 的居中同步帧)。
    #[test]
    fn lyrics_substituted_broken_goes_static_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(64, 14))?;
        let mut state =
            crate::test_support::state_with_lyrics(LyricExtra::None, /*with_words*/ false)?;
        let song_id = state
            .playback
            .track
            .as_ref()
            .map(|s| s.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 应有当前歌"))?;
        state.playback.play_url = Some(mineral_model::PlayUrl {
            song_id,
            url: mineral_model::MediaUrl::remote("https://cdn.example/sub.m4s")?,
            bitrate_bps: 0,
            quality: mineral_model::BitRate::Standard,
            size: 0,
            format: mineral_model::AudioFormat::Aac,
            bit_depth: None,
            stream_headers: Vec::new(),
            layout: mineral_model::StreamLayout::Chunked,
            substituted: true,
        });
        // 元数据时长 vs 实测差 30s:确定失真。直读元数据(`duration_ms()` 对顶换流
        // 会随 engine 值切口径,拿它算会自指)。
        let meta_ms = state.playback.track.as_ref().map_or(0, |t| t.duration_ms);
        state.playback.engine_duration_ms = meta_ms + 30_000;
        t.draw(|f| {
            super::draw(
                f,
                f.area(),
                &state,
                &Theme::default(),
                super::LyricMode::Compact,
            )
        })?;
        crate::test_support::assert_snap!(
            "歌词:顶换流时长失真 → 放弃同步(静态篇首 + unsynced 标识)",
            t.backend()
        );
        Ok(())
    }

    /// 右上副歌词档 + 按键提示;无副歌词时为空(不显示)。
    #[test]
    fn title_right_hint() {
        let th = Theme::default();
        assert_eq!(text_of(&title_right_spans(false, None, &th)), "");
        assert_eq!(
            text_of(&title_right_spans(
                false,
                Some(LyricExtra::Translation),
                &th
            )),
            ""
        );
        assert_eq!(text_of(&title_right_spans(true, None, &th)), " [t] ");
        assert_eq!(
            text_of(&title_right_spans(true, Some(LyricExtra::None), &th)),
            " [t] "
        );
        assert_eq!(
            text_of(&title_right_spans(true, Some(LyricExtra::Translation), &th)),
            " tr · [t] "
        );
        assert_eq!(
            text_of(&title_right_spans(
                true,
                Some(LyricExtra::Romanization),
                &th
            )),
            " ro · [t] "
        );
    }
}
