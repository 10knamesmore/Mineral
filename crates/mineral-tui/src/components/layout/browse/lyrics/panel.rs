//! Lyrics 面板:按本次歌词输入渲染当前行和邻近行,
//! 当前行高亮居中,上下各若干行 dim。无歌词时 fallback "♪ no lyrics"。
//!
//! 有字词时间轴时，按歌词提供的字词跟唱：状态变化触发颜色动画，
//! 当前单元渐入提亮的强调色，唱完退为较暗的正文色；退出当前行也从当时的颜色淡出。
//!
//! `t` 键打开副歌词(翻译 / 罗马音)后，每个可见原文行下方紧跟一条副行；
//! 副行不随播放焦点变色，只保留随距离淡出的层级。

use ratatui::Frame;
use ratatui::layout::{Alignment, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use mineral_config::LyricTextAlphaConfig;
use mineral_model::LyricLine;

use super::sweep::LyricPaint;

use super::{LyricsInput, LyricsPanel, LyricsView};
use crate::components::layout::shared::text::center_bg;
use crate::render::anim::ease_in_out;
use crate::render::color::{lerp_color, lerp_permille};
use crate::render::control_press;
use crate::render::theme::{Ink, Theme, permille_of};
use crate::runtime::format::format_ms;
use crate::runtime::playback::SyncTrust;
use crate::runtime::state::LyricExtra;

/// 渲染 lyrics 面板到给定 [`Rect`]。
///
/// # Params:
///   - `motion`: 呈现模式。[`LyricMode::Compact`] 给嵌入面板(紧凑 + 瞬时高亮);
///     [`LyricMode::Immersive`] 给全屏(行间距 + 缓动平移 + 高亮交叉淡入)。
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &LyricsView<'_>,
    theme: &Theme,
    motion: LyricMode,
) {
    let window = WindowLayout::for_panel(area, state, motion);
    let lines = state
        .input
        .lyrics
        .map(|lyrics| lyrics.lines.as_slice())
        .filter(|v| !v.is_empty());
    let extra = state.active_extra();
    let trust = state.input.trust;

    // 面板 chrome(边框 / 标题弱化色)按实际背景现算:氛围场上贴场色;无人铺 bg 采到
    // Reset 按 base 混合,ANSI 主题回落静态 token(回落链见 Theme::text_over)。
    let ink = theme.ink_over(center_bg(frame, area));
    let press_strength = state.panel.extra_press.strength();
    let press_bg = (press_strength > 0).then(|| {
        let key_area = Rect::new(area.right().saturating_sub(4).max(area.x), area.y, 1, 1);
        control_press::background(center_bg(frame, key_area), press_strength, theme)
    });
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ink.faint))
        .title(Line::from(title_left_spans(
            lines.is_some_and(mineral_model::has_words),
            lines.is_some_and(mineral_model::has_timed),
            trust,
            theme,
            ink,
        )))
        .title_top(
            Line::from(title_right_spans(
                state.has_extra(),
                extra,
                theme,
                ink,
                press_bg,
            ))
            .right_aligned(),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let Some(window) = window.as_ref() else {
        draw_fallback(frame, inner, ink);
        return;
    };
    let lyric_paint =
        state
            .panel
            .colors
            .begin(state.frame.config.tui().lyrics(), motion, state.frame.now);
    paint_window(frame, inner, window, theme, &lyric_paint);
}

/// 歌词准备所需的播放输入、显示模式和只读背景采样。
pub(crate) struct LyricsPreparation<'a> {
    /// 当前歌曲的歌词与播放位置。
    pub(crate) playback: LyricsInput<'a>,

    /// 本次准备的是紧凑还是沉浸面板。
    pub(crate) mode: LyricMode,

    /// 对已确定氛围场采样，不借用可变画布。
    pub(crate) background: &'a dyn Fn(Rect) -> Color,
}

/// 根据可见歌词和背景采样更新字词颜色；不生成任何字符格。
impl crate::components::lifecycle::Prepare for LyricsPanel {
    type Input<'a> = LyricsPreparation<'a>;

    fn prepare(
        &mut self,
        area: Rect,
        preparation: LyricsPreparation<'_>,
        cx: &mut crate::components::frame::PrepareCx<'_>,
    ) {
        let input = preparation.playback;
        let frame = cx.frame;
        let motion = preparation.mode;
        let background = preparation.background;
        let state = self.view(input, frame);
        let theme = frame.theme;
        let targets =
            WindowLayout::for_panel(area, &state, motion).map_or_else(Vec::new, |window| {
                let mut targets = Vec::new();
                for row in &window.rows {
                    let Cell::Primary { line_idx } = row.cell else {
                        continue;
                    };
                    let Some(line) = window.lines.get(line_idx) else {
                        continue;
                    };
                    let words = line.kind.words();
                    if words.is_empty() {
                        continue;
                    }
                    let row_bg = background(row.area);
                    let base = primary_base(row, window.ctx, window.denom, theme, row_bg);
                    let inactive = if Some(line_idx) == window.ctx.focus {
                        lerp_color(base, theme.text, 1, 2)
                    } else {
                        base
                    };
                    targets.extend(super::sweep::word_targets(
                        line_idx,
                        words,
                        (Some(line_idx) == window.ctx.cur).then_some(window.ctx.position_ms),
                        inactive,
                        theme,
                        row_bg,
                        state.frame.config.tui().lyrics(),
                    ));
                }
                targets
            });
        let song = input.song;
        self.colors.prepare(
            song,
            frame.config.tui().lyrics(),
            motion,
            targets,
            frame.now,
        );
    }
}

/// 左上标识:数据档(`lyrics` / `synced` / `synced ✦`)× 时间轴信任档。两档同步用
/// 不同高亮区分——行级 `synced` 用 accent_2(sapphire),逐字 `synced ✦` 用 accent
/// (mauve);`lyrics · ` 前缀恒弱化(strong 档)。顶换流:[`SyncTrust::Borrowed`] 在
/// synced 后缀 `~`(yellow,「可能漂移」);[`SyncTrust::Broken`] 整档换成
/// `unsynced`(yellow,同步已放弃)。
///
/// # Params:
///   - `has_words`: 是否有逐字歌词
///   - `has_lrc`: 是否有行级 LRC
///   - `trust`: 时间轴信任档(无 LRC 时无同步可言,不参与)
///   - `theme`: 取色
///   - `ink`: 对实际背景现算的弱化色阶(前缀 / 空隙用 strong 档)
///
/// # Return:
///   组成 ` lyrics · synced ✦ ` 的分色 Span 序列(首尾留空格)。
fn title_left_spans(
    has_words: bool,
    has_lrc: bool,
    trust: SyncTrust,
    theme: &Theme,
    ink: Ink,
) -> Vec<Span<'static>> {
    let base = Style::new().fg(ink.strong);
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
/// (green)、罗马音标 `ro`(peach),均 bold + italic;`[t]` 及分隔点弱化(muted 档)。
/// 没有任何副歌词可切换时返回空序列(不显示提示)。
///
/// # Params:
///   - `has_extra`: 是否有任一副歌词(翻译 / 罗马音)可切换
///   - `extra`: 当前生效(且非空)的副歌词档;`None` 只显示按键
///   - `theme`: 取色
///   - `ink`: 对实际背景现算的弱化色阶(按键提示 / 分隔点用 muted 档)
///   - `press_bg`: 按压期间的按键底色；空闲时保留原背景
///
/// # Return:
///   组成 ` tr · [t] ` / ` [t] ` 的分色 Span 序列;无副歌词时为空。
fn title_right_spans(
    has_extra: bool,
    extra: Option<LyricExtra>,
    theme: &Theme,
    ink: Ink,
    press_bg: Option<Color>,
) -> Vec<Span<'static>> {
    if !has_extra {
        return Vec::new();
    }
    let key = Style::new().fg(ink.muted);
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
    spans.push(Span::styled("[t]", press_bg.map_or(key, |bg| key.bg(bg))));
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

/// 没歌词时居中渲染一行 `♪ no lyrics`(muted 档 + 斜体)。
fn draw_fallback(frame: &mut Frame<'_>, inner: Rect, ink: Ink) {
    let centered_y = inner.y + inner.height / 2;
    let text_area = Rect::new(inner.x, centered_y, inner.width, 1);
    let line =
        Line::from("♪ no lyrics").style(Style::new().fg(ink.muted).add_modifier(Modifier::ITALIC));
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

    /// 当前播放位置(ms)，用于判断字词的演唱状态。
    position_ms: u64,

    /// 歌词原文的明暗层级。
    text_alpha: &'a LyricTextAlphaConfig,

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

/// 按面板尺寸排好的可见歌词与高亮上下文。
struct WindowLayout<'a> {
    /// 当前曲目的原文行。
    lines: &'a [LyricLine],

    /// 已完成行距、自动跟随和手动滚动定位的可见视觉行。
    rows: Vec<WindowRow>,

    /// 歌词自身的高亮与逐字进度。
    ctx: CellCtx<'a>,

    /// 距离淡色的归一化分母。
    denom: u64,
}

/// 已落在窗口内的视觉行；原文、副歌词和空行都保留各自占位。
struct WindowRow {
    /// 结构化内容，原文行仍以索引标识。
    cell: Cell,

    /// 单行实际屏幕区域。
    area: Rect,

    /// 距窗口中心的行数。
    dist: u64,
}

impl<'a> WindowLayout<'a> {
    /// 按端点模式现读配置和歌词状态；无内容或内区为空时没有窗口。
    fn for_panel(area: Rect, state: &'a LyricsView<'a>, motion: LyricMode) -> Option<Self> {
        let inner = Block::new().borders(Borders::ALL).inner(area);
        if inner.width == 0 || inner.height == 0 {
            return None;
        }
        let lines = state
            .input
            .lyrics
            .map(|lyrics| lyrics.lines.as_slice())
            .filter(|lines| !lines.is_empty())?;
        let position_ms = state.input.position_ms;
        // 失真的时间轴保留静态内容和手动滚动，不生成播放高亮。
        let cur = match state.input.trust {
            SyncTrust::Broken => None,
            SyncTrust::Native | SyncTrust::Borrowed => {
                mineral_model::current_line(lines, position_ms)
            }
        };
        Some(layout_window(
            inner,
            WindowInput {
                lines,
                cur,
                position_ms,
                text_alpha: state.frame.config.tui().lyrics().text_alpha(),
                extra: state.active_extra(),
                motion,
                fullscreen_line_gap: *state.frame.config.tui().lyrics().fullscreen_line_gap(),
                compact_line_gap: *state.frame.config.tui().lyrics().compact_line_gap(),
                scroll_ms: *state.frame.config.tui().lyrics().scroll_ms(),
                // 紧凑面板恒附着，不继承全屏手动偏移。
                manual_anchor_milli: match motion {
                    LyricMode::Immersive => state.manual_anchor(),
                    LyricMode::Compact => None,
                },
                manual_focus: match motion {
                    LyricMode::Immersive => state.manual_focus(),
                    LyricMode::Compact => None,
                },
            },
        ))
    }
}

/// 展开副歌词与行距，按自动或手动锚点定位可见行；稳态与形变共用。
fn layout_window(inner: Rect, input: WindowInput<'_>) -> WindowLayout<'_> {
    let WindowInput {
        lines,
        cur,
        position_ms,
        text_alpha,
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
        text_alpha,
    };

    let height = usize::from(inner.height);
    let center_row = height / 2;
    // fade 用:从中心向两侧最远距离的较大者(窗口可能不对称)。
    let max_dist = u64::try_from(center_row.max(height.saturating_sub(center_row + 1)))
        .unwrap_or(0)
        .max(1);
    let denom = max_dist.saturating_sub(1).max(1);

    let rows = cells
        .into_iter()
        .enumerate()
        .filter_map(|(cell_idx, cell)| {
            // anchor 对应窗口中心，副行与 Spacer 同样占据视觉行。
            let row =
                isize::try_from(cell_idx).ok()? - anchor + isize::try_from(center_row).ok()?;
            let row = u16::try_from(row).ok()?;
            if row >= inner.height {
                return None;
            }
            Some(WindowRow {
                cell,
                area: Rect::new(inner.x, inner.y + row, inner.width, 1),
                dist: u64::try_from(usize::from(row).abs_diff(center_row)).ok()?,
            })
        })
        .collect::<Vec<_>>();
    WindowLayout {
        lines,
        rows,
        ctx,
        denom,
    }
}

/// 歌词随窗口滚动，焦点时间固定在中央光标高度。
fn paint_window(
    frame: &mut Frame<'_>,
    inner: Rect,
    window: &WindowLayout<'_>,
    theme: &Theme,
    lyric_paint: &LyricPaint<'_>,
) {
    let cursor_y = inner.y + inner.height / 2;
    let cursor_time = window
        .ctx
        .focus
        .and_then(|index| window.lines.get(index))
        .and_then(|line| line.time_ms)
        .map(format_ms)
        .filter(|text| text.len() + 2 < usize::from(inner.width / 2));
    for row in &window.rows {
        let row_bg = center_bg(frame, row.area);
        let line = render_cell(
            row,
            window.lines,
            window.ctx,
            window.denom,
            theme,
            row_bg,
            lyric_paint,
        );
        frame.render_widget(
            Paragraph::new(line).alignment(Alignment::Center),
            cursor_time
                .as_ref()
                .filter(|_| row.area.y == cursor_y)
                .map_or(row.area, |text| {
                    // 左右等宽留白
                    #[allow(clippy::as_conversions, reason = "formar_ms has limited length")]
                    row.area.inner(Margin::new(text.len() as u16 + 2, 0))
                }),
        );
    }
    if let Some(text) = cursor_time {
        #[allow(clippy::as_conversions, reason = "formar_ms has limited length")]
        let width = text.len() as u16;
        let time_area = Rect::new(inner.right() - width - 1, cursor_y, width, 1);
        let ink = theme.ink_over(center_bg(frame, time_area));
        frame.render_widget(
            Paragraph::new(text)
                .style(Style::new().fg(ink.strong))
                .alignment(Alignment::Right),
            time_area,
        );
    }
}

/// 渲染一个视觉行所需的高亮上下文(打包以压参数数)。
#[derive(Clone, Copy)]
struct CellCtx<'a> {
    /// 当前行 line index;`None` = 前奏未进首句。
    cur: Option<usize>,

    /// 上一行 line index(交叉淡入的退场端);`None` = 无上一行 / Compact 不淡入。
    prev: Option<usize>,

    /// 脱离态锚定行(手动浏览焦点);`None` = 附着态。当前行优先于焦点行。
    focus: Option<usize>,

    /// 已缓动进度千分比:当前行淡入程度,上一行按 `1000 - eased` 退场。
    eased: u16,

    /// 当前播放位置(ms)，用于判断字词的演唱状态。
    position_ms: u64,

    /// 歌词原文的明暗层级。
    text_alpha: &'a LyricTextAlphaConfig,
}

/// 原文行的距离弱化色，布局准备与绘制使用同一输入。
fn primary_base(
    row: &WindowRow,
    ctx: CellCtx<'_>,
    denom: u64,
    theme: &Theme,
    row_bg: Color,
) -> Color {
    let dist = row.dist;
    let alpha = lerp_permille(
        permille_of(*ctx.text_alpha.neighbor()),
        permille_of(*ctx.text_alpha.distant()),
        dist.saturating_sub(1),
        denom,
    );
    theme.text_over(row_bg, alpha).unwrap_or_else(|| {
        lerp_color(
            theme.surface1,
            theme.surface0,
            dist.saturating_sub(1),
            denom,
        )
    })
}

/// 把一个视觉行渲成 [`Line`]:当前行高亮 / wipe,上一行交叉淡出,其余原文行按距中心 dim,
/// 副歌词仅随距离弱化，空行渲空。
fn render_cell<'a>(
    row: &'a WindowRow,
    lines: &'a [LyricLine],
    ctx: CellCtx<'_>,
    denom: u64,
    theme: &Theme,
    row_bg: Color,
    lyric_paint: &LyricPaint<'_>,
) -> Line<'a> {
    let dist = row.dist;
    match &row.cell {
        Cell::Spacer => Line::default(),
        Cell::Secondary { text } => {
            let alpha = lerp_permille(
                theme.text_alpha.faint,
                theme.text_alpha.ghost,
                dist.saturating_sub(1),
                denom,
            );
            let color = theme.text_over(row_bg, alpha).unwrap_or_else(|| {
                lerp_color(
                    theme.surface1,
                    theme.surface0,
                    dist.saturating_sub(1),
                    denom,
                )
            });
            Line::from(text.as_str()).style(Style::new().fg(color).add_modifier(Modifier::ITALIC))
        }
        Cell::Primary { line_idx } => {
            let line = lines.get(*line_idx);
            let words = line.map(|l| l.kind.words()).filter(|w| !w.is_empty());
            // 其余按 emphasis 在距离淡色与高亮色间插值：当前行淡入、上一行淡出，
            // 手动浏览焦点保持半程，其它行仅用距离淡色。
            let emphasis = if Some(*line_idx) == ctx.cur {
                ctx.eased
            } else if Some(*line_idx) == ctx.prev {
                SCROLL_FULL.saturating_sub(ctx.eased)
            } else if Some(*line_idx) == ctx.focus {
                SCROLL_FULL / 2
            } else {
                0
            };
            // 邻行应比当前行未唱部分更淡，避免切行时反而变暗。
            let base = primary_base(row, ctx, denom, theme, row_bg);
            let mut rendered = if let Some(words) = words {
                let inactive = if Some(*line_idx) == ctx.focus {
                    lerp_color(base, theme.text, 1, 2)
                } else {
                    base
                };
                lyric_paint.line(
                    *line_idx,
                    words,
                    (Some(*line_idx) == ctx.cur).then_some(ctx.position_ms),
                    inactive,
                    theme,
                    row_bg,
                )
            } else {
                let color = lerp_color(
                    base,
                    lyric_paint.highlight(theme),
                    u64::from(emphasis),
                    u64::from(SCROLL_FULL),
                );
                let mut style = Style::new().fg(color);
                // 行级歌词过半激活才加粗；字词颜色动画保持字重不变。
                if emphasis > SCROLL_FULL / 2 {
                    style = style.add_modifier(Modifier::BOLD);
                }
                let text = line.map(|l| l.kind.text().into_owned()).unwrap_or_default();
                Line::from(text).style(style)
            };
            // 脱离态焦点行:文字两侧垫一对淡 `-`(muted 档对本行实际背景混合)作 seek 游标,
            // 提示「Enter 跳到此行」;跟居中文字一起居中故左右对称。当前行 / 上一行不标——
            // 它们已有 now-playing / 交叉淡出线索,与半程焦点行拉开层级。
            if Some(*line_idx) == ctx.focus
                && Some(*line_idx) != ctx.cur
                && Some(*line_idx) != ctx.prev
            {
                let dash = Style::new().fg(theme
                    .text_over(row_bg, theme.text_alpha.muted)
                    .unwrap_or(theme.overlay));
                rendered.spans.insert(0, Span::styled("-  ", dash));
                rendered.spans.push(Span::styled("  -", dash));
            }
            rendered
        }
    }
}
