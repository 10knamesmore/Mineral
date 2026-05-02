//! Spectrum 频谱面板:FFT 真值条 + peak hold cap 装饰 + baseline 兜底。
//!
//! 数据由 [`mineral_spectrum::SpectrumComputer`] 算出 64 根条目标高度,
//! [`SpectrumState::tick`] 7:3 平滑写入。装饰两件:
//!
//! 1. **Peak cap**:每根条记一个 peak,瞬间跟涨,顶部 hold 一段时间再缓慢下落。
//!    渲染为浅色 ▔ 横线浮在条顶上方一格,经典 KTV / Winamp 风格。
//! 2. **Baseline**:任何状态下条高都不低于 [`BASELINE_MIN`],面板永远不死寂。
//!    pause 时条衰减到 baseline 停住,FFT 还没出第一窗时也是 baseline。

use std::cell::Cell;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::color::{lerp_color, rotate_hue};
use crate::theme::Theme;

/// 频谱柱条的逻辑分辨率(每格 1/8 字符高度,共 8 行 × 8 = 64 单位)。
const SPECTRUM_RES: u16 = mineral_spectrum::RES;

/// 首帧 / 重启时的默认条数。`paint_bars` 第一次跑后被实际 area.width 推算的值覆盖。
const DEFAULT_BAR_COUNT: usize = 64;

/// 任何状态下条的最小高度(1/8 字符单位)。3/64 ≈ 5%,屏上是一条薄但可辨的底线。
const BASELINE_MIN: u16 = 3;

/// 新 peak 跟涨后,在原位 hold 多少 tick 才开始下落。30fps 下 12 tick ≈ 400ms。
/// 太短看不出"悬浮"感,太长则跟不上下一个 peak。
const PEAK_HOLD_TICKS: u8 = 12;

/// hold 结束后每 tick peak 下落多少单位。1 单位 = 1/8 字符,2 → 64 单位 32 tick ≈ 1s 落到底,
/// 比条本身的下落慢、但不至于"卡在天上"。
const PEAK_FALL_PER_TICK: u16 = 2;

/// 是否显示 peak cap(`▔` 浮在条顶)。后续接 config 时改读配置。
const SHOW_PEAK_CAP: bool = true;

/// 是否显示 trail(peak 与 bar 之间的余韵 fade)。后续接 config 时改读配置。
const SHOW_TRAIL: bool = true;

/// 是否启用色相缓慢漂移(整渐变在 HSV 色环上慢慢转一圈)。后续接 config 时改读配置。
const HUE_ROTATE: bool = true;

/// 色相旋转一整圈(360°)的 tick 数。30fps 下 1800 tick ≈ 60s,刚好"看着不静止
/// 又不晃眼"。短了会让人头晕,长了一首歌都察觉不到。
const HUE_CYCLE_TICKS: u32 = 1800;

/// 是否启用 peak 弹簧物理(target 跳变时 pos 过冲 + 阻尼回弹)。后续接 config 时改读配置。
const SPRING_PEAK: bool = true;

/// 弹簧刚度。每 tick `force += STIFFNESS × (target - pos)`。0.35 attack 偏强,
/// 跟得上 FFT 跳变;太大瞬间冲过头看着像 bug。
const SPRING_STIFFNESS: f32 = 0.35;

/// 速度阻尼。每 tick `force -= DAMPING × velocity`。
/// 临界阻尼 c = 2·√k ≈ 1.18,这里 0.45 < 临界 → underdamped,2 次可见过冲后稳定。
/// 加大就稳得快但失去"弹"的感觉,减小则振荡多到分散注意。
const SPRING_DAMPING: f32 = 0.45;

/// 频谱状态:每根条的当前高度 + peak target/hold/弹簧 pos+vel + 色相相位。
///
/// peak 拆两层:`peaks[i]` 是 hold/fall 状态机算出的"目标"高度,`peak_pos[i]`
/// 是显示位置(弹簧追目标)。SPRING_PEAK=false 时 peak_pos 直接锁到 peaks。
#[derive(Clone, Debug)]
pub struct SpectrumState {
    /// 当前条高(平滑后),0..=[`SPECTRUM_RES`]。长度 = 当前 bar_count。
    bars: Vec<u16>,

    /// peak 目标高度(hold/fall 状态机维护),0..=[`SPECTRUM_RES`]。peaks[i] >= bars[i] 恒成立。
    peaks: Vec<u16>,

    /// 每根条剩余 hold tick 数。归零后 peak target 开始下落。
    peak_hold: Vec<u8>,

    /// peak 显示位置(弹簧追 peaks 的 target)。可短暂超过 RES(过冲),渲染时 clamp。
    peak_pos: Vec<f32>,

    /// peak 弹簧速度。每 tick 由刚度 / 阻尼推进。
    peak_vel: Vec<f32>,

    /// 色相旋转相位,0..[`HUE_CYCLE_TICKS`]。每 tick +1,渲染时换算成度数。
    hue_phase: u32,

    /// 渲染层根据 area.width 算出的目标条数,FFT compute 下一帧用它。
    /// `Cell` 是因为 `paint_bars` 拿 `&SpectrumState`,这是「render → tick」反向通道。
    pub target_bars: Cell<usize>,
}

impl SpectrumState {
    /// 初始静默状态。所有条都在 baseline,peak target/pos 同位,弹簧速度 0,色相 0。
    pub fn new() -> Self {
        Self {
            bars: vec![BASELINE_MIN; DEFAULT_BAR_COUNT],
            peaks: vec![BASELINE_MIN; DEFAULT_BAR_COUNT],
            peak_hold: vec![0; DEFAULT_BAR_COUNT],
            peak_pos: vec![f32::from(BASELINE_MIN); DEFAULT_BAR_COUNT],
            peak_vel: vec![0.0; DEFAULT_BAR_COUNT],
            hue_phase: 0,
            target_bars: Cell::new(DEFAULT_BAR_COUNT),
        }
    }

    /// 输入 bars 长度变化(终端 resize / 首次 tick)时,把所有 per-bar 状态 vec 调到同长度。
    /// 缩短截断,扩张补 baseline。peak 状态丢一截在缩短时不可避免,resize 是低频事件不在意。
    fn resize_state(&mut self, n: usize) {
        if self.bars.len() == n {
            return;
        }
        self.bars.resize(n, BASELINE_MIN);
        self.peaks.resize(n, BASELINE_MIN);
        self.peak_hold.resize(n, 0);
        self.peak_pos.resize(n, f32::from(BASELINE_MIN));
        self.peak_vel.resize(n, 0.0);
    }

    /// 当前色相旋转角度(度)。`HUE_ROTATE = false` 时恒 0。
    #[allow(clippy::as_conversions)]
    fn hue_deg(&self) -> f32 {
        if !HUE_ROTATE {
            return 0.0;
        }
        // u32 → f32 在这两个量级(< 1800)内精确,允许 as。
        (self.hue_phase as f32) * 360.0 / (HUE_CYCLE_TICKS as f32)
    }

    /// `col` 列的弹簧后 peak 显示位置,clamp 到 `0..=RES` 再 round 成 u16。
    /// 过冲时 raw `peak_pos` 会短暂超过 RES,这里截到上限不让条画出面板外。
    #[allow(clippy::as_conversions)]
    fn spring_peak_at(&self, col: usize) -> u16 {
        let raw = self.peak_pos.get(col).copied().unwrap_or(0.0);
        let clamped = raw.clamp(0.0, f32::from(SPECTRUM_RES));
        clamped.round() as u16
    }

    /// 一次 tick:推进条高 + peak。
    ///
    /// `volume_pct` 用于把 FFT 真值按 `vol/100` 缩放 —— 听感上"音量越大、条越高"。
    /// FFT tap 在 rodio set_volume 之前,信号本身不随音量变,所以这里 UI 层手动配。
    ///
    /// - `Some(targets)`:FFT 真值,按音量缩放后 7:3 平滑写进当前条高(attack 平滑、避免抖动)。
    /// - `None` + `playing=true`:FFT 还没出第一个窗(刚开播 / 切歌 ~43ms),保持当前值。
    /// - `None` + `playing=false`:所有条按 `b - (b/4 + 1)` 衰减(指数+常数,落得快)。
    ///
    /// 然后无条件:1) 把条托底到 [`BASELINE_MIN`];2) 推进 peak 状态机。
    pub fn tick(&mut self, playing: bool, volume_pct: u8, bars: Option<&[u16]>) {
        if let Some(targets) = bars {
            self.resize_state(targets.len());
        }
        match (bars, playing) {
            (Some(targets), _) => {
                let vol = u32::from(volume_pct.min(100));
                for (b, t) in self.bars.iter_mut().zip(targets.iter()) {
                    let scaled = u32::from(*t) * vol / 100;
                    let target = u16::try_from(scaled).unwrap_or(*t);
                    let blended = (u32::from(*b) * 7 + u32::from(target) * 3) / 10;
                    *b = u16::try_from(blended).unwrap_or(*b);
                }
            }
            (None, false) => {
                for b in &mut self.bars {
                    *b = b.saturating_sub(*b / 4 + 1);
                }
            }
            (None, true) => {
                // 还没拉到第一窗,保持上一帧值。
            }
        }
        self.apply_baseline();
        self.advance_peaks();
        self.advance_peak_spring();
        if HUE_ROTATE {
            self.hue_phase = (self.hue_phase + 1) % HUE_CYCLE_TICKS;
        }
    }

    /// 弹簧推进:`peak_pos` 朝 `peaks` (target) 跑,带 [`SPRING_STIFFNESS`] / [`SPRING_DAMPING`]。
    /// `SPRING_PEAK=false` 时直接锁定到 target,无过冲。
    fn advance_peak_spring(&mut self) {
        if !SPRING_PEAK {
            for (pos, p) in self.peak_pos.iter_mut().zip(self.peaks.iter()) {
                *pos = f32::from(*p);
            }
            return;
        }
        for ((pos, vel), p) in self
            .peak_pos
            .iter_mut()
            .zip(self.peak_vel.iter_mut())
            .zip(self.peaks.iter().copied())
        {
            let target = f32::from(p);
            let force = SPRING_STIFFNESS * (target - *pos) - SPRING_DAMPING * *vel;
            *vel += force;
            *pos += *vel;
        }
    }

    /// 把每根条托到 [`BASELINE_MIN`] 之上。静默 / 起播间隙都靠这条保住"面板没死"。
    fn apply_baseline(&mut self) {
        for b in &mut self.bars {
            if *b < BASELINE_MIN {
                *b = BASELINE_MIN;
            }
        }
    }

    /// 推进每根 peak:跟涨瞬间归位 + 重置 hold;否则 hold 倒计时;
    /// hold 归零后按 [`PEAK_FALL_PER_TICK`] 下落,但不跌破当前 bar。
    fn advance_peaks(&mut self) {
        for ((b, p), h) in self
            .bars
            .iter()
            .copied()
            .zip(self.peaks.iter_mut())
            .zip(self.peak_hold.iter_mut())
        {
            if b >= *p {
                *p = b;
                *h = PEAK_HOLD_TICKS;
            } else if *h > 0 {
                *h -= 1;
            } else {
                *p = p.saturating_sub(PEAK_FALL_PER_TICK).max(b);
            }
        }
    }
}

impl Default for SpectrumState {
    fn default() -> Self {
        Self::new()
    }
}

/// 渲染频谱到给定 [`Rect`]。
pub fn draw(frame: &mut Frame<'_>, area: Rect, state: &SpectrumState, theme: &Theme) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(" spectrum ").style(Style::new().fg(theme.subtext)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let labels_h = if inner.height >= 3 { 1 } else { 0 };
    let bars_h = inner.height.saturating_sub(labels_h);
    let bars_area = Rect::new(inner.x, inner.y, inner.width, bars_h);
    paint_bars(frame, bars_area, state, theme);

    if labels_h == 1 {
        let label_area = Rect::new(inner.x, inner.y + bars_h, inner.width, 1);
        paint_labels(frame, label_area, theme);
    }
}

fn paint_bars(frame: &mut Frame<'_>, area: Rect, state: &SpectrumState, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    // 每根条恒 1 列。FFT 端按 area.width 对数等分桶映射,窗口越宽频率分辨率越细。
    let bar_step: u16 = 1;
    let bar_count = usize::from(area.width).max(1);
    state.target_bars.set(bar_count);
    let total_w = u16::try_from(bar_count)
        .unwrap_or(0)
        .saturating_mul(bar_step);
    // 总宽除不尽时把剩余空格平分到两边,让 spectrum 视觉上居中,避免「左密右稀 / 右密左稀」。
    let pad_left = area.width.saturating_sub(total_w) / 2;
    // state.bars 长度可能跟新 bar_count 不一致(刚 resize 终端,新 FFT 还没出第一窗),
    // 这一帧只渲染已有的部分,其余留空。下一帧 tick 后对齐。
    let render_count = bar_count.min(state.bars.len()).max(1);
    let max_units = u32::from(area.height) * 8;
    // 渐变跨度。0 → accent(底)、span → accent_2(顶)。area.height-1 给最顶格 100% accent_2。
    let grad_span = u64::from(area.height.saturating_sub(1)).max(1);
    // 色相旋转后的渐变两端。每帧算一次,不进 col 循环。
    let hue = state.hue_deg();
    let palette_lo = rotate_hue(theme.accent, hue);
    let palette_hi = rotate_hue(theme.accent_2, hue);
    let buf = frame.buffer_mut();
    for col in 0..render_count {
        let bar = state.bars.get(col).copied().unwrap_or(0);
        let peak = state.spring_peak_at(col);
        let scaled = (u32::from(bar) * max_units) / u32::from(SPECTRUM_RES);
        let full = u16::try_from(scaled / 8).unwrap_or(0);
        let partial = u16::try_from(scaled % 8).unwrap_or(0);
        let peak_scaled = (u32::from(peak) * max_units) / u32::from(SPECTRUM_RES);
        let peak_row = u16::try_from(peak_scaled / 8).unwrap_or(0);
        // bar 顶部所占格(partial > 0 时是 full 行;否则 bar 仅到 full-1 的实心格)。
        let bar_top_row = if partial > 0 {
            full
        } else {
            full.saturating_sub(1)
        };
        // trail 区间 = (bar_top_row, peak_row),即 peak 落下时留在空中的「记忆」。
        // trail_span 包含 peak 自身那格,作为 fade 分母:让最顶 trail 行刚好落在
        // 接近(但不到)peak cap 的色阶,色阶逐行递进,无密度跳变。
        let trail_span = u64::from(peak_row.saturating_sub(bar_top_row)).max(1);
        let x = area.x + pad_left + u16::try_from(col).unwrap_or(0) * bar_step;
        for row_from_bottom in 0..area.height {
            let row_color = lerp_color(
                palette_lo,
                palette_hi,
                u64::from(row_from_bottom),
                grad_span,
            );
            let (glyph, color) = if row_from_bottom < full {
                ("█", row_color)
            } else if row_from_bottom == full && partial > 0 {
                (partial_glyph(partial), row_color)
            } else if SHOW_TRAIL && row_from_bottom > bar_top_row && row_from_bottom < peak_row {
                // 余韵:每行往背景色 lerp 一档,d=1 略淡、靠近 peak 几乎融入背景。
                // 单一 glyph(▓)+ 颜色 fade,避免「▓→▒→░」三段密度跳变看起来分层。
                let d = u64::from(row_from_bottom.saturating_sub(bar_top_row));
                let faded = lerp_color(row_color, theme.surface0, d, trail_span);
                ("▓", faded)
            } else {
                continue;
            };
            let y = area.y + area.height.saturating_sub(1 + row_from_bottom);
            for dx in 0..bar_step {
                buf.set_string(x + dx, y, glyph, Style::new().fg(color));
            }
        }

        // peak cap:▔ + theme.text + Bold,跟 bar / trail 的 mauve↔sapphire 拉开。
        // 仅当 peak 严格高于 bar 顶部所占的格才画,避免覆盖 partial glyph 丢失高度信息。
        if SHOW_PEAK_CAP && peak_row > bar_top_row && peak_row < area.height {
            let py = area.y + area.height.saturating_sub(1 + peak_row);
            for dx in 0..bar_step {
                buf.set_string(
                    x + dx,
                    py,
                    "▔",
                    Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
                );
            }
        }
    }
}

fn partial_glyph(units: u16) -> &'static str {
    match units {
        1 => "▁",
        2 => "▂",
        3 => "▃",
        4 => "▄",
        5 => "▅",
        6 => "▆",
        _ => "▇",
    }
}

fn paint_labels(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    if area.width < 12 {
        return;
    }
    let spaces = " ".repeat(usize::from(area.width).saturating_sub(9));
    let line = Line::from(format!("20Hz{spaces}20kHz")).style(Style::new().fg(theme.overlay));
    frame.render_widget(Paragraph::new(line), area);
}
