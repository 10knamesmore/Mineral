//! 溢出标题的循环滚动切片(marquee):把 styled spans 视作首尾相接的循环文本
//! (中间夹 gap 分隔),从给定显示列起取一窗宽度。CJK 双宽字符切在半格时用空格补位。
//!
//! 纯函数层:滚动相位(offset 从哪来、何时重置)由 runtime 状态层负责,此处只做切片。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::components::layout::shared::text::{char_width, display_width};
use crate::render::theme::Theme;
use crate::runtime::marquee::{Marquees, Slot};
use crate::runtime::state::AppState;

/// 一次 title marquee 渲染的共享上下文:相位状态 + gap / fade 配置(从 [`AppState`] 摘取)。
pub(crate) struct MarqueeCtx<'a> {
    /// 相位状态(槽 → 起始拍)。
    pub(crate) marquees: &'a Marquees,

    /// 循环拼接处的分隔串(配置 `animation.marquee_gap`)。
    pub(crate) gap: &'a str,

    /// gap 的样式(overlay 暗色,与 alias 后缀同层)。
    pub(crate) gap_style: Style,

    /// 窗口边缘 fade 的目标色:字符滚近边缘时前景向它插值,取所在渲染位的底色。
    /// **注**:表格选中行的 span 前景会被 ratatui `row_highlight_style` 的 fg 整行
    /// 覆盖(cell 渲染后才 set_style),fade 在表格里实际显不出来——保留整行 accent
    /// 是刻意取舍;fade 只在无行高亮的渲染位(transport / now_playing)可见。
    pub(crate) fade_to: Color,

    /// 边缘 fade 的空间宽度(列,配置 `animation.marquee_fade_cols`)。
    pub(crate) fade_cols: u16,
}

impl<'a> MarqueeCtx<'a> {
    /// 从应用状态摘取 marquee 上下文。
    ///
    /// # Params:
    ///   - `fade_to`: 边缘 fade 目标色(所在渲染位的底色)
    pub(crate) fn new(state: &'a AppState, theme: &Theme, fade_to: Color) -> Self {
        let marquee = state.cfg.tui().animation().marquee();
        Self {
            marquees: &state.marquees,
            gap: marquee.loop_().gap(),
            gap_style: Style::new().fg(theme.overlay),
            fade_to,
            fade_cols: *marquee.fade_cols(),
        }
    }

    /// 组一行可滚动标题:溢出按槽相位循环滚动(带边缘 fade),不溢出原样返回。
    ///
    /// # Params:
    ///   - `spans`: 已组装的标题 spans(各段样式切片后保留)
    ///   - `slot`: 渲染位
    ///   - `identity`: 当前显示对象身份(歌的 `qualified()` id)
    ///   - `window_w`: 可用窗口宽(列)
    ///
    /// # Return:
    ///   可直接喂 `Cell` / `Paragraph` 的 [`Line`]。
    pub(crate) fn line(
        &self,
        spans: Vec<Span<'static>>,
        slot: Slot,
        identity: &str,
        window_w: u16,
    ) -> Line<'static> {
        let content_w = spans
            .iter()
            .map(|s| u32::from(display_width(&s.content)))
            .sum::<u32>();
        let phase = self.marquees.phase(
            slot,
            identity,
            u16::try_from(content_w).unwrap_or(u16::MAX),
            window_w,
            display_width(self.gap),
        );
        let fade = (phase.fade_permille > 0).then_some(EdgeFade {
            to: self.fade_to,
            permille: phase.fade_permille,
            cols: u32::from(self.fade_cols),
        });
        marquee_line(
            spans,
            phase.offset,
            window_w,
            &Span::styled(self.gap.to_owned(), self.gap_style),
            fade.as_ref(),
        )
    }
}

/// 滚动窗口的边缘 fade:边缘 `cols` 列内的字符前景向 `to` 色插值,
/// 越靠边越暗,配合时间渐入强度 `permille` 缓缓出现(不突变)。
pub(crate) struct EdgeFade {
    /// 淡出目标色(所在渲染位的底色)。
    pub(crate) to: Color,

    /// 时间渐入强度(0..=1000,来自 `Marquees::phase`)。
    pub(crate) permille: u16,

    /// 空间宽度(列,配置 `animation.marquee_fade_cols`);0 = 不淡。
    pub(crate) cols: u32,
}

impl EdgeFade {
    /// 对距窗口边缘 `d` 列(0 = 最边缘)的字符样式应用 fade。
    ///
    /// 空间强度取 `(cols - d) / (cols + 1)`——最边缘 cols/(cols+1) 而非全暗,
    /// 字形仍可辨;`d ≥ cols` 原样。最终强度再乘时间渐入系数。
    fn apply(&self, style: Style, d: u32) -> Style {
        let Some(fg) = style.fg else {
            return style;
        };
        if d >= self.cols {
            return style;
        }
        let spatial = (self.cols - d) * 1000 / (self.cols + 1);
        let strength = spatial * u32::from(self.permille) / 1000;
        style.fg(lerp_color(fg, self.to, strength))
    }
}

/// 前景色向目标色按千分比插值;任一侧非 RGB(用户配 ANSI 主题)时原样返回,
/// 良性降级为无 fade。
///
/// # Params:
///   - `from` / `to`: 两端颜色
///   - `permille`: 插值强度(0 = `from`,1000 = `to`),超界钳到 1000
fn lerp_color(from: Color, to: Color, permille: u32) -> Color {
    let (Color::Rgb(r0, g0, b0), Color::Rgb(r1, g1, b1)) = (from, to) else {
        return from;
    };
    let t = permille.min(1000);
    let mix = |a: u8, b: u8| -> u8 {
        let v = u32::from(a) * (1000 - t) + u32::from(b) * t;
        u8::try_from(v / 1000).unwrap_or(a.max(b))
    };
    Color::Rgb(mix(r0, r1), mix(g0, g1), mix(b0, b1))
}

/// 曲目行 title 的 marquee 接线(调用方仅对光标选中行传 `Some`;
/// ♫ 在播行刻意不滚——非焦点处常驻动画干扰,想读全长歌名把光标移过去即可)。
pub(crate) struct RowMarquee<'a> {
    /// 共享上下文。
    pub(crate) ctx: &'a MarqueeCtx<'a>,

    /// 该行所属槽。
    pub(crate) slot: Slot,

    /// title 列实际宽度(经 [`resolve_column_widths`] 解算)。
    pub(crate) title_w: u16,
}

/// 仅光标选中行给 marquee 接线,其余行(含 ♫ 在播行)`None` 维持截断。
///
/// # Params:
///   - `is_selected`: 该行是否为光标选中行
///   - `ctx`: 共享上下文
///   - `slot`: 该表的选中槽
///   - `title_w`: title 列实际宽度
pub(crate) fn row_marquee<'a>(
    is_selected: bool,
    ctx: &'a MarqueeCtx<'a>,
    slot: Slot,
    title_w: u16,
) -> Option<RowMarquee<'a>> {
    is_selected.then_some(RowMarquee { ctx, slot, title_w })
}

/// 解算 Table 各列的实际宽度(镜像 ratatui `Table::get_columns_widths` 的求解:
/// 先切出选中符列,余下按 `Flex::Start` + 列间距 1 求解——仓内所有表都用默认间距)。
///
/// marquee 切片需要预知 title 列宽,而 ratatui 只在渲染内部解算不外露;此处用同款
/// `Layout` 求解器复算。若 ratatui 升级改了内部算法,对照测试会红,失败模式良性
/// (切多/少一列,Cell 兜底截断)。
///
/// # Params:
///   - `total_w`: 表格内容区总宽(inner,扣掉边框后)
///   - `widths`: 列宽约束(与喂给 `Table::new` 的同一组)
///   - `selection_w`: 选中符宽(有选中行时为 `highlight_symbol` 显示宽,否则 0)
///
/// # Return:
///   与 `widths` 等长的各列实际宽度。
pub(crate) fn resolve_column_widths(
    total_w: u16,
    widths: &[Constraint],
    selection_w: u16,
) -> Vec<u16> {
    let [_selection, columns_area] =
        Layout::horizontal([Constraint::Length(selection_w), Constraint::Fill(0)])
            .areas(Rect::new(0, 0, total_w, 1));
    Layout::horizontal(widths.iter().copied())
        .spacing(1)
        .split(columns_area)
        .iter()
        .map(|r| r.width)
        .collect()
}

/// 循环滚动切片:内容溢出窗口时,把 `spans` + `gap` 视作首尾相接的循环文本,
/// 从显示列 `offset`(模周期)起取恰 `window` 列;不溢出时原样返回(忽略 `offset`)。
///
/// CJK 双宽字符切在窗口边界半格时,用空格补其可见的那一列(宽度守恒,列不塌)。
/// 零宽字符(组合符等)不占列,直接跳过。
///
/// # Params:
///   - `spans`: 已组装的标题 spans(各段样式在切片后保留)
///   - `offset`: 滚动相位(显示列数,可超周期,内部取模)
///   - `window`: 可用窗口宽(显示列数)
///   - `gap`: 循环拼接处的分隔 span(内容 + 样式一体;空内容 = 首尾直接相接)
///   - `fade`: 边缘 fade;`None` 不淡。头侧仅在相位非 0(开头已滚出)时淡——
///     停顿期显示的是文本真开头,左缘没有隐藏内容,变暗会误导
///
/// # Return:
///   切片后的 [`Line`];溢出时总显示宽恒等于 `window`。
pub(crate) fn marquee_line(
    spans: Vec<Span<'static>>,
    offset: u16,
    window: u16,
    gap: &Span<'static>,
    fade: Option<&EdgeFade>,
) -> Line<'static> {
    let content_w = spans
        .iter()
        .map(|s| u32::from(display_width(&s.content)))
        .sum::<u32>();
    if content_w <= u32::from(window) {
        return Line::from(spans);
    }
    if window == 0 {
        return Line::default();
    }

    let cycle_w = content_w + u32::from(display_width(&gap.content));
    let start = u32::from(offset) % cycle_w;
    let end = start + u32::from(window);
    // 距边缘列数 → 淡出后的样式;头侧在相位 0 时按「无穷远」处理(不淡)。
    let faded = |style: Style, ch_start: u32, ch_end: u32| -> Style {
        let Some(f) = fade else {
            return style;
        };
        let d_head = if start == 0 {
            u32::MAX
        } else {
            ch_start.max(start) - start
        };
        let d_tail = end - ch_end.min(end);
        f.apply(style, d_head.min(d_tail))
    };

    // 循环序列 = spans 字符流 + gap 字符流;end < 2*cycle_w,展开两轮扫描必覆盖。
    let mut out = SpanRuns::default();
    let mut pos = 0u32;
    for lap in 0..2u32 {
        let _ = lap;
        for span in spans.iter().chain(std::iter::once(gap)) {
            for ch in span.content.chars() {
                let w = u32::from(char_width(ch));
                if w == 0 {
                    continue;
                }
                let (ch_start, ch_end) = (pos, pos + w);
                pos = ch_end;
                if ch_end <= start {
                    continue;
                }
                if ch_start >= end {
                    return out.into_line();
                }
                if ch_start >= start && ch_end <= end {
                    out.push(ch, faded(span.style, ch_start, ch_end));
                } else {
                    // 跨窗口边界的半字:可见列数补等宽空格。头部别改成「回退对齐整字」
                    // ——试过:双宽字消失时同一画面停两拍,比一列空格闪更卡。
                    let visible = ch_end.min(end) - ch_start.max(start);
                    for _ in 0..visible {
                        out.push(' ', span.style);
                    }
                }
            }
        }
    }
    out.into_line()
}

/// 按样式连续段聚合字符成 spans(切片输出的组装缓冲)。
#[derive(Default)]
struct SpanRuns {
    /// 已封口的段。
    done: Vec<Span<'static>>,

    /// 当前累积中的段文本。
    buf: String,

    /// 当前段样式;`None` = 还没有任何字符。
    style: Option<Style>,
}

impl SpanRuns {
    /// 追加一个字符;样式与当前段不同则封口开新段。
    fn push(&mut self, ch: char, style: Style) {
        if self.style != Some(style) {
            self.seal();
            self.style = Some(style);
        }
        self.buf.push(ch);
    }

    /// 封口当前段(空段跳过)。
    fn seal(&mut self) {
        if let Some(style) = self.style.take()
            && !self.buf.is_empty()
        {
            self.done
                .push(Span::styled(std::mem::take(&mut self.buf), style));
        }
    }

    /// 收尾出 [`Line`]。
    fn into_line(mut self) -> Line<'static> {
        self.seal();
        Line::from(self.done)
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Constraint;
    use ratatui::style::{Color, Style};
    use ratatui::text::{Line, Span};
    use unicode_width::UnicodeWidthStr;

    use crate::runtime::marquee::{Marquees, Slot};

    use super::{EdgeFade, MarqueeCtx, lerp_color, marquee_line, resolve_column_widths};

    /// MarqueeCtx::line:溢出时按槽相位滚动——tick 推进后窗口内容前移;不溢出原样。
    #[test]
    fn ctx_line_scrolls_with_ticks() {
        let mut marquees = Marquees::test_loop(/*step_ticks*/ 1, /*pause_ticks*/ 0);
        let make_line = |m: &Marquees| {
            let ctx = MarqueeCtx {
                marquees: m,
                gap: "·",
                gap_style: Style::new(),
                fade_to: Color::Rgb(0, 0, 0),
                fade_cols: 3,
            };
            ctx.line(
                vec![Span::raw("abcdef")],
                Slot::Transport,
                "id",
                /*window_w*/ 4,
            )
        };
        assert_eq!(line_text(&make_line(&marquees)), "abcd", "建档帧显示开头");
        marquees.tick();
        assert_eq!(line_text(&make_line(&marquees)), "bcde", "tick 推进滚 1 列");
    }

    /// MarqueeCtx::line:不溢出原样返回(不裁剪、不拼 gap)。
    #[test]
    fn ctx_line_fitting_passthrough() {
        let marquees = Marquees::test_loop(/*step_ticks*/ 1, /*pause_ticks*/ 0);
        let ctx = MarqueeCtx {
            marquees: &marquees,
            gap: "·",
            gap_style: Style::new(),
            fade_to: Color::Rgb(0, 0, 0),
            fade_cols: 3,
        };
        let line = ctx.line(
            vec![Span::raw("abc")],
            Slot::Transport,
            "id",
            /*window_w*/ 10,
        );
        assert_eq!(line_text(&line), "abc");
    }

    /// 颜色插值:0 = 原色、1000 = 目标色、500 = 中点;非 RGB 原样返回(良性降级)。
    #[test]
    fn lerp_color_endpoints_and_fallback() {
        let a = Color::Rgb(200, 100, 0);
        let b = Color::Rgb(0, 100, 200);
        assert_eq!(lerp_color(a, b, 0), a);
        assert_eq!(lerp_color(a, b, 1000), b);
        assert_eq!(lerp_color(a, b, 500), Color::Rgb(100, 100, 100));
        assert_eq!(lerp_color(Color::Red, b, 500), Color::Red, "非 RGB 原样");
    }

    /// 边缘 fade:滚动中(相位非 0)两侧边缘字符前景向目标色靠拢、中间字符原色;
    /// 相位 0(停顿期显示真开头)时左缘不淡、右缘照淡。
    #[test]
    fn edge_fade_dims_edges_only() -> color_eyre::Result<()> {
        let fg = Color::Rgb(200, 200, 200);
        let to = Color::Rgb(0, 0, 0);
        let fade = EdgeFade {
            to,
            permille: 1000,
            cols: 3,
        };
        let brightness = |line: &Line<'_>, ch: char| -> color_eyre::Result<u8> {
            line.spans
                .iter()
                .find(|s| s.content.contains(ch))
                .and_then(|s| match s.style.fg {
                    Some(Color::Rgb(r, _, _)) => Some(r),
                    _ => None,
                })
                .ok_or_else(|| color_eyre::eyre::eyre!("找不到字符 {ch}"))
        };
        // 滚动中:窗口 8,内容 "abcdefghijkl"(12),offset 1 → 显示 bcdefghi。
        let line = marquee_line(
            vec![Span::styled("abcdefghijkl", Style::new().fg(fg))],
            /*offset*/ 1,
            /*window*/ 8,
            &Span::raw("·"),
            Some(&fade),
        );
        let head = brightness(&line, 'b')?;
        let mid = brightness(&line, 'e')?;
        let tail = brightness(&line, 'i')?;
        assert!(head < mid, "左缘应比中间暗: head={head} mid={mid}");
        assert!(tail < mid, "右缘应比中间暗: tail={tail} mid={mid}");
        assert_eq!(mid, 200, "中间字符应保持原色");

        // 停顿期(offset 0):左缘是文本真开头,不淡;右缘照淡。
        let paused = marquee_line(
            vec![Span::styled("abcdefghijkl", Style::new().fg(fg))],
            /*offset*/ 0,
            /*window*/ 8,
            &Span::raw("·"),
            Some(&fade),
        );
        assert_eq!(brightness(&paused, 'a')?, 200, "相位 0 左缘不该淡");
        assert!(brightness(&paused, 'h')? < 200, "相位 0 右缘仍该淡");
        Ok(())
    }

    /// 时间渐入强度减半 → 边缘变暗幅度也减半(空间 × 时间两级系数相乘)。
    #[test]
    fn edge_fade_scales_with_ramp_in() -> color_eyre::Result<()> {
        let fg = Color::Rgb(200, 200, 200);
        let to = Color::Rgb(0, 0, 0);
        let edge_r = |permille: u16| -> color_eyre::Result<u8> {
            let line = marquee_line(
                vec![Span::styled("abcdefghijkl", Style::new().fg(fg))],
                /*offset*/ 1,
                /*window*/ 8,
                &Span::raw("·"),
                Some(&EdgeFade {
                    to,
                    permille,
                    cols: 3,
                }),
            );
            line.spans
                .first()
                .and_then(|s| match s.style.fg {
                    Some(Color::Rgb(r, _, _)) => Some(r),
                    _ => None,
                })
                .ok_or_else(|| color_eyre::eyre::eyre!("无首 span"))
        };
        let full = edge_r(1000)?;
        let half = edge_r(500)?;
        assert!(
            full < half && half < 200,
            "渐入半程的边缘应介于原色与满强度之间: full={full} half={half}"
        );
        Ok(())
    }

    /// 手算对照:总宽 40、选中列 2,定宽 1+4+6 + 三处 spacing 1 → Fill 列得 24。
    #[test]
    fn resolve_widths_matches_hand_calc() {
        let widths = [
            Constraint::Length(1),
            Constraint::Length(4),
            Constraint::Fill(1),
            Constraint::Length(6),
        ];
        assert_eq!(
            resolve_column_widths(/*total_w*/ 40, &widths, /*selection_w*/ 2),
            vec![1, 4, 24, 6]
        );
    }

    /// 与 ratatui Table 真实渲染对照(防内部算法漂移):按解算宽度截断的 title
    /// 应与 Table 自己画出来的列边界一致——解算宽度内的字符都在、下一个字符不在。
    #[test]
    fn resolve_widths_matches_real_table_render() -> color_eyre::Result<()> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::widgets::{Cell, Row, Table, TableState};

        let widths = [
            Constraint::Length(1),
            Constraint::Length(4),
            Constraint::Fill(1),
            Constraint::Length(6),
        ];
        let title_w = resolve_column_widths(/*total_w*/ 40, &widths, /*selection_w*/ 2)
            .get(2)
            .copied()
            .ok_or_else(|| color_eyre::eyre::eyre!("缺 title 列"))?;
        let long = "abcdefghijklmnopqrstuvwxyz0123456789";
        let rows = vec![Row::new(vec![
            Cell::from("♥"),
            Cell::from("0"),
            Cell::from(long),
            Cell::from("3:45"),
        ])];
        let mut t = Terminal::new(TestBackend::new(40, 2))?;
        let mut state = TableState::default();
        state.select(Some(0));
        t.draw(|f| {
            f.render_stateful_widget(
                Table::new(rows, widths).highlight_symbol("▌ "),
                f.area(),
                &mut state,
            );
        })?;
        let buf = t.backend().buffer();
        let row_text = (0..buf.area.width)
            .filter_map(|x| buf.cell((x, 0)).map(ratatui::buffer::Cell::symbol))
            .collect::<String>();
        let visible = long.chars().take(usize::from(title_w)).collect::<String>();
        let over = long
            .chars()
            .take(usize::from(title_w) + 1)
            .collect::<String>();
        assert!(
            row_text.contains(&visible),
            "解算宽度内的 title 字符应完整可见: {row_text}"
        );
        assert!(
            !row_text.contains(&over),
            "解算宽度外的下一字符不应出现(列边界一致): {row_text}"
        );
        Ok(())
    }

    /// 把 Line 的各 span 内容拼成纯文本(断言可读性 helper)。
    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// Line 的总显示宽度(CJK 双宽)。
    fn line_width(line: &Line<'_>) -> usize {
        UnicodeWidthStr::width(line_text(line).as_str())
    }

    /// 造一个默认样式的测试 gap span。
    fn gap(s: &str) -> Span<'static> {
        Span::raw(s.to_owned())
    }

    /// 内容不溢出窗口 → 原样返回,不拼 gap、不滚动(offset 无效)。
    #[test]
    fn fits_returns_unchanged() {
        let line = marquee_line(
            vec![Span::raw("abc")],
            /*offset*/ 7,
            /*window*/ 10,
            &gap("·"),
            /*fade*/ None,
        );
        assert_eq!(line_text(&line), "abc", "不溢出应原样返回且忽略 offset");
    }

    /// 溢出 + offset 0:显示头部恰一窗宽。
    #[test]
    fn offset_zero_shows_head() {
        let line = marquee_line(
            vec![Span::raw("abcdefgh")],
            /*offset*/ 0,
            /*window*/ 5,
            &gap("·"),
            /*fade*/ None,
        );
        assert_eq!(line_text(&line), "abcde");
    }

    /// 滚过末尾回绕:尾部 + gap + 头部拼成一窗。
    #[test]
    fn wraps_through_gap() {
        let line = marquee_line(
            vec![Span::raw("abcdef")],
            /*offset*/ 4,
            /*window*/ 5,
            &gap("·"),
            /*fade*/ None,
        );
        // 循环序列 "abcdef·" 从列 4 起取 5 列 → e f · a b
        assert_eq!(line_text(&line), "ef·ab");
    }

    /// offset 超过一个循环周期取模回绕(相位等价)。
    #[test]
    fn offset_wraps_modulo_cycle() {
        let base = marquee_line(
            vec![Span::raw("abcdef")],
            /*offset*/ 4,
            /*window*/ 5,
            &gap("·"),
            /*fade*/ None,
        );
        let wrapped = marquee_line(
            vec![Span::raw("abcdef")],
            /*offset*/ 4 + 7, // cycle = 6 + 1
            /*window*/ 5,
            &gap("·"),
            /*fade*/ None,
        );
        assert_eq!(line_text(&wrapped), line_text(&base), "offset 应模周期等价");
    }

    /// CJK 双宽字符切在半格:头尾跨界都补等宽空格,相位严格随 offset 匀速推进。
    /// 别改成「头部回退对齐整字」——试过:双宽字消失时同一画面停两拍,比空格闪更卡。
    #[test]
    fn cjk_half_glyph_padded_with_space() {
        // 循环序列 "迷星叫 "(宽 7),offset 1 起取 4 列:
        // 列 1 = 迷后半(补空格)、列 2-3 = 星、列 4 = 叫前半(补空格)。
        let line = marquee_line(
            vec![Span::raw("迷星叫")],
            /*offset*/ 1,
            /*window*/ 4,
            &gap(" "),
            /*fade*/ None,
        );
        assert_eq!(line_text(&line), " 星 ");
        assert_eq!(line_width(&line), 4, "半字补位后宽度仍应恰等于窗口");
    }

    /// 多 span 样式保留:name 亮 / alias 暗 / gap 自带样式,切片后各段前景色不串。
    #[test]
    fn styles_survive_slicing() -> color_eyre::Result<()> {
        let name_style = Style::new().fg(Color::White);
        let alias_style = Style::new().fg(Color::DarkGray);
        let gap_style = Style::new().fg(Color::Blue);
        let line = marquee_line(
            vec![
                Span::styled("abcd", name_style),
                Span::styled("XY", alias_style),
            ],
            /*offset*/ 3,
            /*window*/ 5,
            &Span::styled("·", gap_style),
            /*fade*/ None,
        );
        // 循环序列 "abcdXY·" 从列 3 起取 5 列 → d X Y · a
        assert_eq!(line_text(&line), "dXY·a");
        let style_at = |ch: &str| -> Option<Style> {
            line.spans
                .iter()
                .find(|s| s.content.contains(ch))
                .map(|s| s.style)
        };
        assert_eq!(style_at("d"), Some(name_style), "name 段样式应保留");
        assert_eq!(style_at("X"), Some(alias_style), "alias 段样式应保留");
        assert_eq!(style_at("·"), Some(gap_style), "gap 样式应保留");
        Ok(())
    }

    /// 窗口宽 0 → 空行(布局挤压时不 panic)。
    #[test]
    fn zero_window_yields_empty() {
        let line = marquee_line(
            vec![Span::raw("abcdef")],
            /*offset*/ 2,
            /*window*/ 0,
            &gap("·"),
            /*fade*/ None,
        );
        assert_eq!(line_text(&line), "");
    }

    /// 空 gap:循环周期 = 内容宽,首尾直接相接仍正确回绕。
    #[test]
    fn empty_gap_wraps_directly() {
        let line = marquee_line(
            vec![Span::raw("abcdef")],
            /*offset*/ 4,
            /*window*/ 4,
            &gap(""),
            /*fade*/ None,
        );
        assert_eq!(line_text(&line), "efab");
    }

    use proptest::prelude::*;

    proptest! {
        /// 宽度守恒不变量:溢出时任意内容(ASCII/CJK 混排)、任意 offset,
        /// 输出显示宽恒等于窗口宽(半字补位不多不少)。
        #[test]
        fn prop_overflow_output_width_equals_window(
            text in "[a-z迷星叫音乐]{4,20}",
            offset in 0u16..500,
            window in 1u16..12,
        ) {
            let content_w = UnicodeWidthStr::width(text.as_str());
            prop_assume!(content_w > usize::from(window));
            let line = marquee_line(
                vec![Span::raw(text)],
                offset,
                window,
                &Span::raw("  "),
                /*fade*/ None,
            );
            prop_assert_eq!(line_width(&line), usize::from(window));
        }
    }
}
