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

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};

use crate::render::color::{lerp_color, rotate_hue};
use crate::render::palette::{ColumnColors, CoverPalette};
use crate::render::theme::Theme;

/// 频谱柱条的逻辑分辨率(每格 1/8 字符高度,共 8 行 × 8 = 64 单位)。
const SPECTRUM_RES: u16 = mineral_spectrum::RES;

/// 首帧 / 重启时的默认条数。`paint_bars` 第一次跑后被实际 area.width 推算的值覆盖。
const DEFAULT_BAR_COUNT: usize = 64;

/// 任何状态下条的最小高度(1/8 字符单位)。3/64 ≈ 5%,屏上是一条薄但可辨的底线。
const BASELINE_MIN: u16 = 3;

/// 上升平滑(attack)旧值权重:条高 EMA `(旧×ATTACK_OLD + 新×ATTACK_NEW) / 两者之和`。
const ATTACK_OLD: u32 = 7;

/// 上升平滑(attack)新值权重。占比越大越跟手、越跳;当前 7:3,新值占 30%。
const ATTACK_NEW: u32 = 3;

/// 静默/暂停时条高每 tick 的衰减除数:先减 `b / DECAY_DIV`(指数部分,大值落得快)。
const DECAY_DIV: u16 = 4;

/// 见 [`DECAY_DIV`]:衰减的常数项,叠加在指数项上,保证小值也能落到底、不卡半空。
const DECAY_STEP: u16 = 1;

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

/// 封面就绪后,从当前可见配色缓动到封面色场的过渡时长(tick)。
const COVER_FADE_TICKS: u32 = 300;

/// 2D 色场的纵向采样偏移(‰):顶端点比底端点沿色带多偏向高频多少。
/// 200‰ 在底/顶之间拉开一档明度层次,延续"底暗顶亮"的气质。
const COVER_VSHIFT_PERMILLE: u32 = 200;

/// 是否启用 peak 弹簧物理(target 跳变时 pos 过冲 + 阻尼回弹)。后续接 config 时改读配置。
const SPRING_PEAK: bool = true;

/// 弹簧刚度。每 tick `force += STIFFNESS × (target - pos)`。0.35 attack 偏强,
/// 跟得上 FFT 跳变;太大瞬间冲过头看着像 bug。
const SPRING_STIFFNESS: f32 = 0.35;

/// 速度阻尼。每 tick `force -= DAMPING × velocity`。
/// 临界阻尼 c = 2·√k ≈ 1.18,这里 0.45 < 临界 → underdamped,2 次可见过冲后稳定。
/// 加大就稳得快但失去"弹"的感觉,减小则振荡多到分散注意。
const SPRING_DAMPING: f32 = 0.45;

/// 频谱配色状态机:无封面时沿 hue 漂移,当前播放封面取色就绪后缓动到封面色场再静止。
///
/// 命令只有 [`SpectrumState::begin_cover_transition`] / [`SpectrumState::clear_cover`] 两个,
/// "当前是哪张封面"的身份判定全在 app 层,故本态机能脱离播放器单测。
#[derive(Clone, Debug)]
enum SpectrumColor {
    /// 默认 / 无封面:全列同色,沿用 `hue_phase` 驱动的色相漂移(现状逐像素等价)。
    Hue,

    /// 封面就绪:从**切换那刻的可见配色**缓动到封面色场。`frame` 0→[`COVER_FADE_TICKS`]。
    ///
    /// 起点存整个上一态,故红专辑换蓝专辑时起点是红、不是 hue 初始色。
    Transition {
        /// 过渡起点态(切换那刻的可见配色)。begin 时已扁平化为 `Hue` / `CoverFixed`,
        /// 不嵌套 `Transition`,故起点端点计算最多递归一层、不会无限。
        from: Box<Self>,

        /// `from` 为 `Hue` 时的固定色相角(冻结时刻的 `hue_deg()`);`from` 为 `CoverFixed` 时无用。
        frozen_hue_deg: f32,

        /// 目标封面色场。
        to: CoverPalette,

        /// 已过渡帧数,推进到 [`COVER_FADE_TICKS`] 转入 [`Self::CoverFixed`]。
        frame: u32,
    },

    /// 过渡完成:静止显示封面色场,不再随 tick 变化(hue 停转)。
    CoverFixed {
        /// 静止显示的封面色场。
        palette: CoverPalette,
    },
}

impl SpectrumColor {
    /// 算第 `col` 列(共 `bar_count` 列)的底/顶端点色,垂直 lerp 逻辑由调用方不变沿用。
    ///
    /// - `Hue`:全列同色 `rotate_hue(accent, hue) → rotate_hue(accent_2, hue)`(零回归)。
    /// - `CoverFixed`:沿封面色带按频率位置取底/顶端点(见 [`CoverPalette::column_endpoints`])。
    /// - `Transition`:起点 = `from` 态该列端点(`Hue` 起点全列同色、`CoverFixed` 起点沿色带),
    ///   终点 = 目标色场该列端点,按 `frame/COVER_FADE_TICKS` 整数定点逐端点 lerp。
    ///
    /// # Params:
    ///   - `col`: 列序号(从 0 起)
    ///   - `bar_count`: 总列数
    ///   - `hue_deg`: 当前 `Hue` 态色相角(仅 `Hue` 态用)
    ///   - `theme`: 取 `accent` / `accent_2` 端点
    ///
    /// # Return:
    ///   该列底 / 顶端点色。
    fn column_endpoints(
        &self,
        col: usize,
        bar_count: usize,
        hue_deg: f32,
        theme: &Theme,
    ) -> ColumnColors {
        match self {
            Self::Hue => ColumnColors {
                bottom: rotate_hue(theme.accent, hue_deg),
                top: rotate_hue(theme.accent_2, hue_deg),
            },
            Self::CoverFixed { palette } => {
                palette.column_endpoints(col, bar_count, COVER_VSHIFT_PERMILLE)
            }
            Self::Transition {
                from,
                frozen_hue_deg,
                to,
                frame,
            } => {
                let start = from.column_endpoints(col, bar_count, *frozen_hue_deg, theme);
                let end = to.column_endpoints(col, bar_count, COVER_VSHIFT_PERMILLE);
                let prog = u64::from(*frame).saturating_mul(1000) / u64::from(COVER_FADE_TICKS);
                ColumnColors {
                    bottom: lerp_color(start.bottom, end.bottom, prog, 1000),
                    top: lerp_color(start.top, end.top, prog, 1000),
                }
            }
        }
    }
}

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

    /// 色相旋转相位,0..[`HUE_CYCLE_TICKS`]。仅 `Hue` 态每 tick +1,渲染时换算成度数。
    hue_phase: u32,

    /// 配色状态机。默认 `Hue`(漂移),封面取色就绪后由 app 层命令切到过渡 / 静止。
    color: SpectrumColor,

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
            color: SpectrumColor::Hue,
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
    /// - `Some(targets)`:FFT 真值,按音量缩放后按 [`ATTACK_OLD`]:[`ATTACK_NEW`] 平滑写进当前条高(attack 平滑、避免抖动)。
    /// - `None` + `playing=true`:FFT 还没出第一个窗(刚开播 / 切歌 ~43ms),保持当前值。
    /// - `None` + `playing=false`:所有条按 [`DECAY_DIV`]/[`DECAY_STEP`] 衰减(指数+常数,落得快)。
    ///
    /// 然后无条件:1) 把条托底到 [`BASELINE_MIN`];2) 推进 peak 状态机。
    pub fn tick(&mut self, playing: bool, volume_pct: u8, bars: Option<&[u16]>) {
        match bars {
            Some(targets) => self.resize_state(targets.len()),
            // idle / 起播间隙没有 FFT 真值,仍把条数同步到渲染层反馈的面板宽度,
            // 否则 baseline 只铺满初始 `DEFAULT_BAR_COUNT` 列、宽面板右侧空白。
            None => self.resize_state(self.target_bars.get().max(1)),
        }
        match (bars, playing) {
            (Some(targets), _) => {
                let vol = u32::from(volume_pct.min(100));
                for (b, t) in self.bars.iter_mut().zip(targets.iter()) {
                    let scaled = u32::from(*t) * vol / 100;
                    let target = u16::try_from(scaled).unwrap_or(*t);
                    let blended = (u32::from(*b) * ATTACK_OLD + u32::from(target) * ATTACK_NEW)
                        / (ATTACK_OLD + ATTACK_NEW);
                    *b = u16::try_from(blended).unwrap_or(*b);
                }
            }
            (None, false) => {
                for b in &mut self.bars {
                    *b = b.saturating_sub(*b / DECAY_DIV + DECAY_STEP);
                }
            }
            (None, true) => {
                // 还没拉到第一窗,保持上一帧值。
            }
        }
        self.apply_baseline();
        self.advance_peaks();
        self.advance_peak_spring();
        self.advance_color();
    }

    /// 推进配色状态机一拍(替换原先裸 `hue_phase` 自增):
    ///
    /// - `Hue`:`HUE_ROTATE` 时 `hue_phase` 自增、绕 [`HUE_CYCLE_TICKS`] 取模。
    /// - `Transition`:`frame += 1`,到 [`COVER_FADE_TICKS`] 转 `CoverFixed`(hue 停转)。
    /// - `CoverFixed`:不动。
    ///
    /// 用 `mem::replace` 取出当前态再写回,避免在 `match` 内 move `palette`(无 clone)。
    fn advance_color(&mut self) {
        match std::mem::replace(&mut self.color, SpectrumColor::Hue) {
            SpectrumColor::Hue => {
                if HUE_ROTATE {
                    self.hue_phase = (self.hue_phase + 1) % HUE_CYCLE_TICKS;
                }
                // color 已被换回 Hue,无需再写。
            }
            SpectrumColor::Transition {
                from,
                frozen_hue_deg,
                to,
                frame,
            } => {
                let next = frame + 1;
                self.color = if next >= COVER_FADE_TICKS {
                    SpectrumColor::CoverFixed { palette: to }
                } else {
                    SpectrumColor::Transition {
                        from,
                        frozen_hue_deg,
                        to,
                        frame: next,
                    }
                };
            }
            SpectrumColor::CoverFixed { palette } => {
                self.color = SpectrumColor::CoverFixed { palette };
            }
        }
    }

    /// 命令:封面取色就绪,从**当前可见配色**缓动到封面色场 `to`。
    ///
    /// 起点 = 切换那刻的整个可见态:`Hue` 漂移则从当前 hue 单色起步;已是 `CoverFixed`
    /// (上一张封面)则**从那张封面的色场起步**(红专辑换蓝专辑 → 红→蓝,而非 hue 初始色)。
    /// 已在 `Transition`(换歌打断)时把它压成"行将抵达的目标色场"作起点,避免 `from` 无限嵌套。
    ///
    /// # Params:
    ///   - `to`: 目标封面色场
    pub fn begin_cover_transition(&mut self, to: CoverPalette) {
        let frozen_hue_deg = self.hue_deg();
        // 取出当前可见态作起点(占位换成 Hue);Transition 起点扁平化为其目标 CoverFixed。
        let from = match std::mem::replace(&mut self.color, SpectrumColor::Hue) {
            SpectrumColor::Transition { to: prev_to, .. } => {
                Box::new(SpectrumColor::CoverFixed { palette: prev_to })
            }
            visible => Box::new(visible),
        };
        self.color = SpectrumColor::Transition {
            from,
            frozen_hue_deg,
            to,
            frame: 0,
        };
    }

    /// 命令:无封面 / 取色失败,回到 `Hue` 漂移(`hue_phase` 从当前值继续,不重置)。
    pub fn clear_cover(&mut self) {
        self.color = SpectrumColor::Hue;
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

    let bars_area = Rect::new(inner.x, inner.y, inner.width, inner.height);
    paint_bars(frame, bars_area, state, theme);
}

/// 渲染整个频谱条阵:每列一根柱 + 渐变色 + 余韵尾迹 + peak cap。
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
    // 当前 hue 漂移角度(`Hue` 态全列共用、`Transition` 起点用)。封面态各列端点不同,
    // 故端点计算下沉进 col 循环。
    let hue = state.hue_deg();
    let buf = frame.buffer_mut();
    for col in 0..render_count {
        // 该列底/顶端点色:`Hue` 态全列同色(与旧实现逐像素等价),封面态沿频率轴铺色。
        let endpoints = state.color.column_endpoints(col, render_count, hue, theme);
        let palette_lo = endpoints.bottom;
        let palette_hi = endpoints.top;
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

/// 把 0..=7 单位的剩余高度映射成 8 段块字符(`▁..▇`),用于顶部"半行"渲染。
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

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    use super::{COVER_FADE_TICKS, SpectrumColor, SpectrumState};
    use crate::render::palette::{CoverPalette, Rgb};
    use crate::render::theme::Theme;

    /// 固定 3 色板(暗蓝 / 中红 / 亮黄绿,明度拉开),注入态机 / 色场 / 快照测试。
    fn fixed_palette() -> color_eyre::Result<CoverPalette> {
        CoverPalette::new(vec![
            Rgb::new(20, 20, 120),
            Rgb::new(200, 40, 40),
            Rgb::new(180, 220, 60),
        ])
        .ok_or_else(|| color_eyre::eyre::eyre!("非空应构造成功"))
    }

    /// 频谱 baseline(`SpectrumState::new()`,静默态)渲染快照。
    #[test]
    fn spectrum_baseline_snapshot() -> color_eyre::Result<()> {
        let mut terminal = Terminal::new(TestBackend::new(40, 10))?;
        let state = SpectrumState::new();
        terminal.draw(|f| super::draw(f, f.area(), &state, &Theme::default()))?;
        crate::test_support::assert_snap!("频谱静默基线(SpectrumState::new())", terminal.backend());
        Ok(())
    }

    /// 生成长度 `w` 的确定性参差 bars(值域 8..=57),给"有音频"快照当 mock 频谱形状。
    /// 纯整数运算,各列高度不同 → 渲染出"跳动"起伏;避免浮点转换 lint。
    fn jagged_bars(w: usize) -> Vec<u16> {
        (0..w)
            .map(|i| u16::try_from(8 + (i * 13) % 50).unwrap_or(0))
            .collect::<Vec<u16>>()
    }

    /// 有音频(mock bars)宽面板快照:内宽 78 > 64,验证柱条铺满整宽且高度参差。
    /// 喂同一组 bars 多次 tick 让平滑 / 弹簧收敛到稳态,快照确定。
    #[test]
    fn spectrum_with_audio_full_width_snapshot() -> color_eyre::Result<()> {
        let mut terminal = Terminal::new(TestBackend::new(80, 10))?;
        let mut state = SpectrumState::new();
        // 先 draw 一帧让 paint_bars 把 target_bars 设成真实内宽。
        terminal.draw(|f| super::draw(f, f.area(), &state, &Theme::default()))?;
        let bars = jagged_bars(state.target_bars.get());
        for _ in 0..30 {
            state.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        terminal.draw(|f| super::draw(f, f.area(), &state, &Theme::default()))?;
        crate::test_support::assert_snap!(
            "频谱有音频(mock bars,占满宽度有起伏)",
            terminal.backend()
        );
        Ok(())
    }

    /// 无音频(idle)宽面板快照:内宽 78 > 64,验证 baseline 铺满整宽且各列等高无起伏。
    /// tick 走 `None` 路径,本是宽面板右侧空白 bug 的现场,此处锁住修复后行为。
    #[test]
    fn spectrum_silent_full_width_snapshot() -> color_eyre::Result<()> {
        let mut terminal = Terminal::new(TestBackend::new(80, 10))?;
        let mut state = SpectrumState::new();
        terminal.draw(|f| super::draw(f, f.area(), &state, &Theme::default()))?;
        for _ in 0..30 {
            state.tick(false /*playing*/, 100 /*volume_pct*/, None);
        }
        terminal.draw(|f| super::draw(f, f.area(), &state, &Theme::default()))?;
        crate::test_support::assert_snap!(
            "频谱无音频(idle baseline,占满宽度无起伏)",
            terminal.backend()
        );
        Ok(())
    }

    /// `begin_cover_transition` 后 tick [`COVER_FADE_TICKS`] 次,落到 `CoverFixed`(静止)。
    #[test]
    fn cover_transition_settles_to_fixed() -> color_eyre::Result<()> {
        let mut s = SpectrumState::new();
        s.begin_cover_transition(fixed_palette()?);
        assert!(
            matches!(s.color, SpectrumColor::Transition { .. }),
            "begin 后应进 Transition"
        );
        for _ in 0..COVER_FADE_TICKS {
            s.tick(false /*playing*/, 100 /*volume_pct*/, None);
        }
        assert!(
            matches!(s.color, SpectrumColor::CoverFixed { .. }),
            "过渡满应转入 CoverFixed"
        );
        Ok(())
    }

    /// `clear_cover` 把任意态拉回 `Hue` 漂移。
    #[test]
    fn clear_cover_returns_to_hue() -> color_eyre::Result<()> {
        let mut s = SpectrumState::new();
        s.begin_cover_transition(fixed_palette()?);
        s.clear_cover();
        assert!(matches!(s.color, SpectrumColor::Hue), "clear 后应回 Hue");
        Ok(())
    }

    /// `Hue` 态 tick 推进 `hue_phase`;`CoverFixed` 态 tick 不改色(端点两帧一致)。
    #[test]
    fn hue_advances_but_coverfixed_is_frozen() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let mut s = SpectrumState::new();
        let before = s.hue_phase;
        s.tick(false /*playing*/, 100 /*volume_pct*/, None);
        assert_ne!(s.hue_phase, before, "Hue 态 hue_phase 应自增");

        s.begin_cover_transition(fixed_palette()?);
        for _ in 0..COVER_FADE_TICKS {
            s.tick(false /*playing*/, 100 /*volume_pct*/, None);
        }
        let ep1 = s.color.column_endpoints(2, 16, s.hue_deg(), &theme);
        s.tick(false /*playing*/, 100 /*volume_pct*/, None);
        let ep2 = s.color.column_endpoints(2, 16, s.hue_deg(), &theme);
        assert_eq!(ep1, ep2, "CoverFixed 态 tick 不应改色");
        Ok(())
    }

    /// `Hue` 态:所有列端点相同(与旧单色实现等价,零回归)。
    #[test]
    fn hue_columns_are_uniform() {
        let theme = Theme::default();
        let color = SpectrumColor::Hue;
        let first = color.column_endpoints(
            /*col*/ 0, /*bar_count*/ 16, /*hue*/ 30.0, &theme,
        );
        let mid = color.column_endpoints(
            /*col*/ 8, /*bar_count*/ 16, /*hue*/ 30.0, &theme,
        );
        let last = color.column_endpoints(
            /*col*/ 15, /*bar_count*/ 16, /*hue*/ 30.0, &theme,
        );
        assert_eq!(first, mid);
        assert_eq!(mid, last);
    }

    /// `CoverFixed` 态:底色沿频率轴推进,首列 ≠ 末列。
    #[test]
    fn coverfixed_columns_span_frequency() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let color = SpectrumColor::CoverFixed {
            palette: fixed_palette()?,
        };
        let first = color.column_endpoints(
            /*col*/ 0, /*bar_count*/ 16, /*hue*/ 0.0, &theme,
        );
        let last = color.column_endpoints(
            /*col*/ 15, /*bar_count*/ 16, /*hue*/ 0.0, &theme,
        );
        assert_ne!(first.bottom, last.bottom, "底色应沿频率轴变化");
        Ok(())
    }

    /// `Transition`(从 `Hue` 起步)态:`frame=0` 等于 frozen-hue 起点,`frame=COVER_FADE_TICKS`
    /// 等于 `CoverFixed`。
    #[test]
    fn transition_endpoints_match_start_and_end() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let palette = fixed_palette()?;
        let frozen = 42.0_f32;

        let at_start = SpectrumColor::Transition {
            from: Box::new(SpectrumColor::Hue),
            frozen_hue_deg: frozen,
            to: palette.clone(),
            frame: 0,
        };
        // 起点 = 该列在 frozen_hue_deg 下的 Hue 端点(外层 hue 参数被 Transition 忽略,只用 from)。
        let hue_start = SpectrumColor::Hue
            .column_endpoints(/*col*/ 3, /*bar_count*/ 16, frozen, &theme);
        assert_eq!(
            at_start.column_endpoints(
                /*col*/ 3, /*bar_count*/ 16, /*hue*/ 999.0, &theme
            ),
            hue_start,
            "progress=0 应等于 frozen hue 端点"
        );

        let at_end = SpectrumColor::Transition {
            from: Box::new(SpectrumColor::Hue),
            frozen_hue_deg: frozen,
            to: palette.clone(),
            frame: COVER_FADE_TICKS,
        };
        let fixed = SpectrumColor::CoverFixed { palette };
        assert_eq!(
            at_end.column_endpoints(
                /*col*/ 3, /*bar_count*/ 16, /*hue*/ 0.0, &theme
            ),
            fixed.column_endpoints(
                /*col*/ 3, /*bar_count*/ 16, /*hue*/ 0.0, &theme
            ),
            "progress=1 应等于 CoverFixed 端点"
        );
        Ok(())
    }

    /// 红→蓝:已在 `CoverFixed`(红)时换新色板(蓝),过渡**起点端点 = 红色场**(非 hue 初始色)。
    /// 锁住"当前可见色 → 新色"的预期。
    #[test]
    fn transition_from_cover_starts_at_previous_cover() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let red = CoverPalette::new(vec![
            Rgb::new(120, 20, 20),
            Rgb::new(200, 40, 40),
            Rgb::new(240, 90, 90),
        ])
        .ok_or_else(|| color_eyre::eyre::eyre!("红色板非空"))?;
        let blue = CoverPalette::new(vec![
            Rgb::new(20, 20, 120),
            Rgb::new(40, 40, 200),
            Rgb::new(90, 90, 240),
        ])
        .ok_or_else(|| color_eyre::eyre::eyre!("蓝色板非空"))?;
        let mut s = SpectrumState::new();
        // 先进入红色场静止。
        s.begin_cover_transition(red.clone());
        for _ in 0..COVER_FADE_TICKS {
            s.tick(false /*playing*/, 100 /*volume_pct*/, None);
        }
        assert!(
            matches!(s.color, SpectrumColor::CoverFixed { .. }),
            "应先静止在红"
        );

        // 换蓝:刚切(frame=0)那刻起点端点应等于红色场,而不是 hue。
        s.begin_cover_transition(blue);
        let at_start =
            s.color
                .column_endpoints(/*col*/ 3, /*bar_count*/ 16, s.hue_deg(), &theme);
        let red_endpoints = SpectrumColor::CoverFixed { palette: red }.column_endpoints(
            /*col*/ 3, /*bar_count*/ 16, /*hue*/ 0.0, &theme,
        );
        assert_eq!(
            at_start, red_endpoints,
            "红→蓝起点应是红色场,不是 hue 初始色"
        );
        Ok(())
    }

    /// 集成:`paint_bars` 真的把 per-column 端点接到了渲染。
    ///
    /// 字形快照(`assert_snap!` 走 Display)只抓字符、不抓颜色,故颜色覆盖落在这里:
    /// 直接读渲染后 buffer 底行最左 / 最右 cell 的前景色。`Hue` 态全列同色 → 两端相等;
    /// `CoverFixed` 态沿频率轴铺色 → 两端不等。
    #[test]
    fn paint_bars_wires_per_column_color() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let mut terminal = Terminal::new(TestBackend::new(80, 10))?;
        let mut state = SpectrumState::new();
        // 先渲一帧让 target_bars = 真实内宽,再喂音频让条形铺满底行。
        terminal.draw(|f| super::draw(f, f.area(), &state, &theme))?;
        let bars = jagged_bars(state.target_bars.get());
        for _ in 0..30 {
            state.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }

        terminal.draw(|f| super::draw(f, f.area(), &state, &theme))?;
        let hue = bottom_row_edges(terminal.backend())?;
        assert_eq!(hue.left, hue.right, "Hue 态全列同色,底行两端前景应相等");

        // 注入固定色板并推到静止,沿频率轴铺色。
        state.begin_cover_transition(fixed_palette()?);
        for _ in 0..COVER_FADE_TICKS {
            state.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        terminal.draw(|f| super::draw(f, f.area(), &state, &theme))?;
        let cover = bottom_row_edges(terminal.backend())?;
        assert_ne!(
            cover.left, cover.right,
            "CoverFixed 态沿频率轴铺色,底行两端前景应不同"
        );
        Ok(())
    }

    /// 频谱底行两端列的前景色(测试比较用,命名字段替代 `(Color, Color)`)。
    struct RowEdges {
        /// 最左列(低频)前景。
        left: Color,

        /// 最右列(高频)前景。
        right: Color,
    }

    /// 读频谱底行(最底边框上一行)最左 / 最右列 cell 的前景色。
    ///
    /// # Params:
    ///   - `backend`: 已渲染的 `TestBackend`
    ///
    /// # Return:
    ///   两端列前景;cell 缺失返回 `Err`。
    fn bottom_row_edges(backend: &TestBackend) -> color_eyre::Result<RowEdges> {
        let buf = backend.buffer();
        let area = *buf.area();
        let y = area.height.saturating_sub(2); // 底部边框上一行
        let left_x = area.x + 1; // 左边框右一列
        let right_x = area.x + area.width.saturating_sub(2); // 右边框左一列
        let left = buf
            .cell((left_x, y))
            .ok_or_else(|| color_eyre::eyre::eyre!("最左 cell 缺失"))?
            .fg;
        let right = buf
            .cell((right_x, y))
            .ok_or_else(|| color_eyre::eyre::eyre!("最右 cell 缺失"))?
            .fg;
        Ok(RowEdges { left, right })
    }
}
