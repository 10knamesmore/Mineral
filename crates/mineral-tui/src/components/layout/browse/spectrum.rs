//! Spectrum 频谱面板:FFT 真值条 + peak hold cap 装饰 + baseline 兜底。
//!
//! 数据由 [`mineral_spectrum::SpectrumComputer`] 算出 64 根条目标高度,
//! [`SpectrumState::tick`] 按效果器 ADSR 包络写入:attack(上升)/ decay(播放中
//! 余韵滑落)/ release(暂停释音落 0),sustain 即 FFT 实时值。时长旋钮均为毫秒,
//! 构造时按 `animation.frame_tick_ms` 折算成每拍系数,与帧率解耦。装饰两件:
//!
//! 1. **Peak cap**:每根条记一个 peak,瞬间跟涨,顶部 hold 一段时间再缓慢下落。
//!    渲染为浅色 ▔ 横线浮在条顶上方一格,经典 KTV / Winamp 风格。
//! 2. **Baseline**:任何状态下条高都不低于配置的 `baseline_min`,面板永远不死寂。
//!    pause 时条衰减到 baseline 停住,FFT 还没出第一窗时也是 baseline。

use std::cell::Cell;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};

use crate::render::anim::{ticks16_from_ms, ticks32_from_ms};
use crate::render::color::{lerp_color, rotate_hue};
use crate::render::palette::{ColumnColors, CoverPalette, Rgb, column_permille};
use crate::render::theme::Theme;

/// 频谱柱条的逻辑分辨率(每格 1/8 字符高度,共 8 行 × 8 = 64 单位)。
const SPECTRUM_RES: u16 = mineral_spectrum::RES;

/// 首帧 / 重启时的默认条数。`paint_bars` 第一次跑后被实际 area.width 推算的值覆盖。
const DEFAULT_BAR_COUNT: usize = 64;

/// 打断快照沿频率轴的采样点数(65 点 = 64 段,15.625‰/段)。色场本身是分段线性渐变
/// (色板 swatch ≤ 6),这个密度下弦差不足一个可辨色阶;烘焙只在打断那刻发生一次。
const SNAPSHOT_SAMPLES: usize = 65;

/// 色场计算参数(过渡时长 + 纵向偏移),由 [`SpectrumState`] 从配置派生后传给
/// [`SpectrumColor`] 的端点计算 —— 色彩态机本身不持配置。
#[derive(Clone, Copy, Debug)]
struct ColorParams {
    /// 2D 色场的纵向采样偏移(‰),配置 `spectrum.cover_vshift_permille`。
    vshift_permille: u32,

    /// 封面色场过渡时长(tick),配置 `spectrum.cover_fade_ticks`。
    fade_ticks: u32,
}

/// 频谱配色状态机:无封面时沿 hue 漂移,当前播放封面取色就绪后缓动到封面色场再静止。
///
/// 命令只有 [`SpectrumState::begin_cover_transition`] / [`SpectrumState::clear_cover`] 两个,
/// "当前是哪张封面"的身份判定全在 app 层,故本态机能脱离播放器单测。
#[derive(Clone, Debug)]
enum SpectrumColor {
    /// 默认 / 无封面:全列同色,沿用 `hue_phase` 驱动的色相漂移(现状逐像素等价)。
    Hue,

    /// 封面就绪:从**切换那刻的可见配色**缓动到封面色场。`frame` 0→过渡拍数
    /// (`cover_fade_ms` 按帧率折算)。
    ///
    /// 起点存整个上一态,故红专辑换蓝专辑时起点是红、不是 hue 初始色。
    Transition {
        /// 过渡起点态(切换那刻的可见配色)。begin 时已经 [`Self::freeze`] 冻结为
        /// `Hue` / `CoverFixed` / `Snapshot` 等扁平态,不嵌套 `Transition`,
        /// 故起点端点计算最多递归一层、不会无限。
        from: Box<Self>,

        /// `from` 为 `Hue` 时的固定色相角(冻结时刻的 `hue_deg()`);`from` 为 `CoverFixed` 时无用。
        frozen_hue_deg: f32,

        /// 目标封面色场。
        to: CoverPalette,

        /// 已过渡帧数,推进到过渡拍数(`cover_fade_ms` 折算)转入 [`Self::CoverFixed`]。
        frame: u32,
    },

    /// 过渡完成:静止显示封面色场,不再随 tick 变化(hue 停转)。
    CoverFixed {
        /// 静止显示的封面色场。
        palette: CoverPalette,
    },

    /// 打断快照:过渡中途再次换目标时,"打断那刻"的可见色场沿频率轴均匀采
    /// [`SNAPSHOT_SAMPLES`] 点烘焙成的底/顶两条色带(见 [`Self::freeze`])。
    /// 只作 [`Self::Transition`] 的起点,正常流程不驻留顶层。
    Snapshot {
        /// 各列柱底色构成的色带(频率轴均匀采样)。
        bottom: CoverPalette,

        /// 各列柱顶色构成的色带。烘焙时已含纵向偏移效果,取色不再加 vshift。
        top: CoverPalette,
    },
}

impl SpectrumColor {
    /// 算第 `col` 列(共 `bar_count` 列)的底/顶端点色,垂直 lerp 逻辑由调用方不变沿用。
    ///
    /// - `Hue`:全列同色 `rotate_hue(accent, hue) → rotate_hue(accent_2, hue)`(零回归)。
    /// - `CoverFixed`:沿封面色带按频率位置取底/顶端点(见 [`CoverPalette::column_endpoints`])。
    /// - `Snapshot`:底/顶两条烘焙色带按频率位置各自采样(顶带已含 vshift,不再偏移)。
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
        p: ColorParams,
    ) -> ColumnColors {
        match self {
            Self::Hue => ColumnColors {
                bottom: rotate_hue(theme.accent, hue_deg),
                top: rotate_hue(theme.accent_2, hue_deg),
            },
            Self::CoverFixed { palette } => {
                palette.column_endpoints(col, bar_count, p.vshift_permille)
            }
            Self::Snapshot { bottom, top } => {
                let tx = column_permille(col, bar_count);
                ColumnColors {
                    bottom: bottom.sample(tx),
                    top: top.sample(tx),
                }
            }
            Self::Transition {
                from,
                frozen_hue_deg,
                to,
                frame,
            } => {
                let start = from.column_endpoints(col, bar_count, *frozen_hue_deg, theme, p);
                let end = to.column_endpoints(col, bar_count, p.vshift_permille);
                let prog = u64::from(*frame).saturating_mul(1000) / u64::from(p.fade_ticks.max(1));
                ColumnColors {
                    bottom: lerp_color(start.bottom, end.bottom, prog, 1000),
                    top: lerp_color(start.top, end.top, prog, 1000),
                }
            }
        }
    }

    /// 把当前可见态冻结成可作过渡起点的**扁平**态:
    ///
    /// - `Hue` / `CoverFixed` / `Snapshot`:无随帧推进的内部进度,原样返回(精确)。
    /// - `Transition`(换歌打断):把打断那刻的可见色场沿频率轴均匀采 [`SNAPSHOT_SAMPLES`]
    ///   点,烘焙成底/顶两条色带([`Self::Snapshot`])——新过渡从可见中间色继续渐变,
    ///   不跳回打断目标。任一采样点非真彩(`Color::Rgb`)时无法入带,退化为打断目标色场
    ///   (与真彩路径相比只少了中间色连续性,行为同旧实现)。
    ///
    /// # Params:
    ///   - `theme`: 过渡起点为 `Hue` 时换算端点色用
    ///
    /// # Return:
    ///   扁平态(恒不含 `Transition`)。
    fn freeze(self, theme: &Theme, p: ColorParams) -> Self {
        let Self::Transition { ref to, .. } = self else {
            return self;
        };
        let fallback = to.clone();
        let mut bottoms = Vec::with_capacity(SNAPSHOT_SAMPLES);
        let mut tops = Vec::with_capacity(SNAPSHOT_SAMPLES);
        for col in 0..SNAPSHOT_SAMPLES {
            // Transition 的端点计算只用内部 frozen_hue_deg,外层 hue 参数不参与。
            let ep = self.column_endpoints(col, SNAPSHOT_SAMPLES, /*hue_deg*/ 0.0, theme, p);
            let (Color::Rgb(br, bg, bb), Color::Rgb(tr, tg, tb)) = (ep.bottom, ep.top) else {
                return Self::CoverFixed { palette: fallback };
            };
            bottoms.push(Rgb::new(br, bg, bb));
            tops.push(Rgb::new(tr, tg, tb));
        }
        match (CoverPalette::new(bottoms), CoverPalette::new(tops)) {
            (Some(bottom), Some(top)) => Self::Snapshot { bottom, top },
            // SNAPSHOT_SAMPLES > 0,两条带恒非空;留作类型穷尽。
            _ => Self::CoverFixed { palette: fallback },
        }
    }
}

/// 时间制旋钮(`*_ms`)按 `frame_tick_ms` 折算后的运行时值(构造时算一次,热路径直读)。
#[derive(Clone, Copy, Debug)]
struct Timing {
    /// 起音(上升)每 tick EMA 系数,来自 `attack_ms`。
    alpha_attack: f32,

    /// 衰减(播放中下落)每 tick EMA 系数,来自 `decay_ms`。
    alpha_decay: f32,

    /// 释音(暂停落 0)每 tick EMA 系数,来自 `release_ms`。
    alpha_release: f32,

    /// peak 悬停拍数,来自 `peak_hold_ms`。
    peak_hold_ticks: u16,

    /// peak 每 tick 下落量(1/8 字符单位),来自 `peak_fall_ms`(满程时长)。
    peak_fall_per_tick: f32,

    /// 色相一圈拍数,来自 `hue_cycle_ms`。
    hue_cycle_ticks: u32,

    /// 封面色场过渡拍数,来自 `cover_fade_ms`。
    fade_ticks: u32,
}

impl Timing {
    /// 从配置 + 帧间隔折算全部运行时值。
    ///
    /// # Params:
    ///   - `cfg`: 频谱旋钮(时间制)
    ///   - `tick_ms`: 主循环帧间隔(毫秒,配置 `animation.frame_tick_ms`)
    fn derive(cfg: &mineral_config::SpectrumConfig, tick_ms: u64) -> Self {
        Self {
            alpha_attack: alpha_from_t90(*cfg.attack_ms(), tick_ms),
            alpha_decay: alpha_from_t90(*cfg.decay_ms(), tick_ms),
            alpha_release: alpha_from_t90(*cfg.release_ms(), tick_ms),
            peak_hold_ticks: ticks16_from_ms(*cfg.peak_hold_ms(), tick_ms),
            peak_fall_per_tick: fall_per_tick(*cfg.peak_fall_ms(), tick_ms),
            hue_cycle_ticks: ticks32_from_ms(*cfg.hue_cycle_ms(), tick_ms),
            fade_ticks: ticks32_from_ms(*cfg.cover_fade_ms(), tick_ms),
        }
    }
}

/// 频谱状态:每根条的当前高度 + peak target/hold/弹簧 pos+vel + 色相相位。
///
/// peak 拆两层:`peaks[i]` 是 hold/fall 状态机算出的"目标"高度,`peak_pos[i]`
/// 是显示位置(弹簧追目标)。SPRING_PEAK=false 时 peak_pos 直接锁到 peaks。
#[derive(Clone, Debug)]
pub struct SpectrumState {
    /// 当前条高(ADSR 包络,f32 收敛精确无整数截断),0..=[`SPECTRUM_RES`]。
    /// 长度 = 当前 bar_count;渲染经 [`Self::bar_at`] 收整。
    bars: Vec<f32>,

    /// peak 目标高度(hold/fall 状态机维护),0..=[`SPECTRUM_RES`]。peaks[i] >= bars[i] 恒成立。
    peaks: Vec<f32>,

    /// 每根条剩余 hold tick 数。归零后 peak target 开始下落。
    peak_hold: Vec<u16>,

    /// peak 显示位置(弹簧追 peaks 的 target)。可短暂超过 RES(过冲),渲染时 clamp。
    peak_pos: Vec<f32>,

    /// peak 弹簧速度。每 tick 由刚度 / 阻尼推进。
    peak_vel: Vec<f32>,

    /// 色相旋转相位,0..`hue_cycle_ticks`。仅 `Hue` 态每 tick +1,渲染时换算成度数。
    hue_phase: u32,

    /// 配色状态机。默认 `Hue`(漂移),封面取色就绪后由 app 层命令切到过渡 / 静止。
    color: SpectrumColor,

    /// 渲染层根据 area.width 算出的目标条数,FFT compute 下一帧用它。
    /// `Cell` 是因为 `paint_bars` 拿 `&SpectrumState`,这是「render → tick」反向通道。
    pub target_bars: Cell<usize>,

    /// 频谱旋钮(平滑/衰减/peak 物理/观感开关),构造时由配置注入。
    cfg: mineral_config::SpectrumConfig,

    /// 时间制旋钮折算后的运行时拍数/系数(构造时由 `cfg` + 帧间隔派生)。
    timing: Timing,
}

impl SpectrumState {
    /// 初始静默状态。所有条都在 baseline,peak target/pos 同位,弹簧速度 0,色相 0。
    ///
    /// # Params:
    ///   - `cfg`: 频谱旋钮(配置 `tui.spectrum` 段)
    ///   - `frame_tick_ms`: 主循环帧间隔(毫秒,配置 `animation.frame_tick_ms`);
    ///     时间制旋钮(`*_ms`)按它折算成拍数与每拍系数
    pub fn new(cfg: mineral_config::SpectrumConfig, frame_tick_ms: u64) -> Self {
        let baseline = f32::from(*cfg.baseline_min());
        let timing = Timing::derive(&cfg, frame_tick_ms);
        Self {
            bars: vec![baseline; DEFAULT_BAR_COUNT],
            peaks: vec![baseline; DEFAULT_BAR_COUNT],
            peak_hold: vec![0; DEFAULT_BAR_COUNT],
            peak_pos: vec![baseline; DEFAULT_BAR_COUNT],
            peak_vel: vec![0.0; DEFAULT_BAR_COUNT],
            hue_phase: 0,
            color: SpectrumColor::Hue,
            target_bars: Cell::new(DEFAULT_BAR_COUNT),
            cfg,
            timing,
        }
    }

    /// 配置热更:换旋钮 + 重折时间制拍数,保留条高 / peak / 色相等运行态
    /// (频谱不因改配置闪断)。
    ///
    /// # Params:
    ///   - `cfg`: 新频谱旋钮(配置 `tui.spectrum` 段)
    ///   - `frame_tick_ms`: 主循环帧间隔(毫秒)
    pub fn reconfigure(&mut self, cfg: mineral_config::SpectrumConfig, frame_tick_ms: u64) {
        self.timing = Timing::derive(&cfg, frame_tick_ms);
        self.cfg = cfg;
    }

    /// 从配置派生色场计算参数(传给 [`SpectrumColor`] 的端点计算)。
    fn color_params(&self) -> ColorParams {
        ColorParams {
            vshift_permille: *self.cfg.cover_vshift_permille(),
            fade_ticks: self.timing.fade_ticks,
        }
    }

    /// 输入 bars 长度变化(终端 resize / 首次 tick)时,把所有 per-bar 状态 vec 调到同长度。
    /// 缩短截断,扩张补 baseline。peak 状态丢一截在缩短时不可避免,resize 是低频事件不在意。
    fn resize_state(&mut self, n: usize) {
        if self.bars.len() == n {
            return;
        }
        let baseline = f32::from(*self.cfg.baseline_min());
        self.bars.resize(n, baseline);
        self.peaks.resize(n, baseline);
        self.peak_hold.resize(n, 0);
        self.peak_pos.resize(n, baseline);
        self.peak_vel.resize(n, 0.0);
    }

    /// 当前色相旋转角度(度)。`hue_rotate = false` 时恒 0。
    #[allow(clippy::as_conversions)]
    fn hue_deg(&self) -> f32 {
        if !*self.cfg.hue_rotate() {
            return 0.0;
        }
        // u32 → f32 在这两个量级(典型 < 数千 tick)内精确,允许 as。
        (self.hue_phase as f32) * 360.0 / (self.timing.hue_cycle_ticks as f32).max(1.0)
    }

    /// `col` 列的弹簧后 peak 显示位置,clamp 到 `0..=RES` 再 round 成 u16。
    /// 过冲时 raw `peak_pos` 会短暂超过 RES,这里截到上限不让条画出面板外。
    #[allow(clippy::as_conversions)]
    fn spring_peak_at(&self, col: usize) -> u16 {
        let raw = self.peak_pos.get(col).copied().unwrap_or(0.0);
        let clamped = raw.clamp(0.0, f32::from(SPECTRUM_RES));
        clamped.round() as u16
    }

    /// `col` 列的条高收整(渲染用):clamp 到 `0..=RES` 再 round 成 u16。
    /// 内部包络是 f32(收敛精确、无整数截断),只在渲染口收整。
    #[allow(clippy::as_conversions)]
    fn bar_at(&self, col: usize) -> u16 {
        let raw = self.bars.get(col).copied().unwrap_or(0.0);
        let clamped = raw.clamp(0.0, f32::from(SPECTRUM_RES));
        clamped.round() as u16
    }

    /// 一次 tick:推进条高 + peak。
    ///
    /// `volume_pct` 用于把 FFT 真值按 `vol/100` 缩放 —— 听感上"音量越大、条越高"。
    /// FFT tap 在 rodio set_volume 之前,信号本身不随音量变,所以这里 UI 层手动配。
    ///
    /// 条高走效果器 ADSR 包络(`b += α × (target − b)`,α 由时间制旋钮折算):
    /// - `Some(targets)`:FFT 真值(= sustain),上升用 attack(快、贴鼓点),
    ///   下落用 decay(慢、余韵滑落)——快攻慢放,延迟与动画感分属两个旋钮。
    /// - `None` + `playing=true`:FFT 还没出第一个窗(刚开播 / 切歌),保持当前值。
    /// - `None` + `playing=false`:释音(release),所有条滑向 0(由 baseline 兜底)。
    ///
    /// 然后无条件:1) 把条托底到 `baseline_min`;2) 推进 peak 状态机。
    pub fn tick(&mut self, playing: bool, volume_pct: u8, bars: Option<&[u16]>) {
        match bars {
            Some(targets) => self.resize_state(targets.len()),
            // idle / 起播间隙没有 FFT 真值,仍把条数同步到渲染层反馈的面板宽度,
            // 否则 baseline 只铺满初始 `DEFAULT_BAR_COUNT` 列、宽面板右侧空白。
            None => self.resize_state(self.target_bars.get().max(1)),
        }
        match (bars, playing) {
            (Some(targets), _) => {
                let vol = f32::from(volume_pct.min(100));
                for (b, t) in self.bars.iter_mut().zip(targets.iter()) {
                    let target = f32::from(*t) * vol / 100.0;
                    // 不对称包络:涨用 attack(贴鼓点),跌用 decay(余韵)。
                    let alpha = if target > *b {
                        self.timing.alpha_attack
                    } else {
                        self.timing.alpha_decay
                    };
                    *b += alpha * (target - *b);
                }
            }
            (None, false) => {
                for b in &mut self.bars {
                    *b -= self.timing.alpha_release * *b;
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
    /// - `Hue`:`hue_rotate` 时 `hue_phase` 自增、绕 `hue_cycle_ticks` 取模。
    /// - `Transition`:`frame += 1`,到 `cover_fade_ticks` 转 `CoverFixed`(hue 停转)。
    /// - `CoverFixed`:不动。
    ///
    /// 用 `mem::replace` 取出当前态再写回,避免在 `match` 内 move `palette`(无 clone)。
    fn advance_color(&mut self) {
        match std::mem::replace(&mut self.color, SpectrumColor::Hue) {
            SpectrumColor::Hue => {
                if *self.cfg.hue_rotate() {
                    self.hue_phase = (self.hue_phase + 1) % self.timing.hue_cycle_ticks.max(1);
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
                self.color = if next >= self.timing.fade_ticks {
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
            SpectrumColor::Snapshot { bottom, top } => {
                // 快照只作过渡起点,正常流程不驻留顶层;防御性写回、不推进。
                self.color = SpectrumColor::Snapshot { bottom, top };
            }
        }
    }

    /// 命令:封面取色就绪,从**当前可见配色**缓动到封面色场 `to`。
    ///
    /// 起点 = 切换那刻的整个可见态:`Hue` 漂移则从当前 hue 单色起步;已是 `CoverFixed`
    /// (上一张封面)则**从那张封面的色场起步**(红专辑换蓝专辑 → 红→蓝,而非 hue 初始色)。
    /// 已在 `Transition`(换歌打断)时把打断那刻的可见中间色烘焙成快照作起点
    /// (见 [`SpectrumColor::freeze`])——颜色从中间态继续渐变,不跳回打断前的目标色场。
    ///
    /// # Params:
    ///   - `to`: 目标封面色场
    ///   - `theme`: 冻结起点时换算 `Hue` 端点色用
    pub fn begin_cover_transition(&mut self, to: CoverPalette, theme: &Theme) {
        let frozen_hue_deg = self.hue_deg();
        let params = self.color_params();
        // 取出当前可见态、冻结成扁平起点(占位换成 Hue)。
        let from =
            Box::new(std::mem::replace(&mut self.color, SpectrumColor::Hue).freeze(theme, params));
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

    /// 弹簧推进:`peak_pos` 朝 `peaks` (target) 跑,带配置的刚度 / 阻尼。
    /// `spring_peak=false` 时直接锁定到 target,无过冲。
    fn advance_peak_spring(&mut self) {
        if !*self.cfg.spring_peak() {
            for (pos, p) in self.peak_pos.iter_mut().zip(self.peaks.iter()) {
                *pos = *p;
            }
            return;
        }
        let (stiffness, damping) = (*self.cfg.spring_stiffness(), *self.cfg.spring_damping());
        for ((pos, vel), target) in self
            .peak_pos
            .iter_mut()
            .zip(self.peak_vel.iter_mut())
            .zip(self.peaks.iter().copied())
        {
            let force = stiffness * (target - *pos) - damping * *vel;
            *vel += force;
            *pos += *vel;
        }
    }

    /// 把每根条托到 `baseline_min` 之上。静默 / 起播间隙都靠这条保住"面板没死"。
    fn apply_baseline(&mut self) {
        let baseline = f32::from(*self.cfg.baseline_min());
        for b in &mut self.bars {
            if *b < baseline {
                *b = baseline;
            }
        }
    }

    /// 推进每根 peak:跟涨瞬间归位 + 重置 hold;否则 hold 倒计时;
    /// hold 归零后按 `peak_fall_ms` 折算的每拍量下落,但不跌破当前 bar。
    fn advance_peaks(&mut self) {
        let (hold_ticks, fall) = (self.timing.peak_hold_ticks, self.timing.peak_fall_per_tick);
        for ((b, p), h) in self
            .bars
            .iter()
            .copied()
            .zip(self.peaks.iter_mut())
            .zip(self.peak_hold.iter_mut())
        {
            if b >= *p {
                *p = b;
                *h = hold_ticks;
            } else if *h > 0 {
                *h -= 1;
            } else {
                *p = (*p - fall).max(b);
            }
        }
    }
}

/// `t90`(到位 90% 所需毫秒)→ 每 tick EMA 系数 α:`α = 1 − 0.1^(tick/t90)`。
/// 定义性质:经过 `t90/tick` 拍后残差恰为 10%。`t90 ≤ tick` 时一拍内就该到位,
/// 直接取 1.0(瞬时,无平滑)。
///
/// # Params:
///   - `t90_ms`: 到位 90% 所需毫秒(配置 `attack_ms` / `decay_ms` / `release_ms`)
///   - `tick_ms`: 主循环帧间隔(毫秒)
///
/// # Return:
///   每 tick 追赶系数,`0.0 < α ≤ 1.0`。
#[allow(clippy::as_conversions)] // 纯数值换算:u64 tick(≤ t90 ≤ u32::MAX)在 f64 内精确
fn alpha_from_t90(t90_ms: u32, tick_ms: u64) -> f32 {
    let tick = tick_ms.max(1);
    if u64::from(t90_ms) <= tick {
        return 1.0;
    }
    let ratio = (tick as f64) / f64::from(t90_ms);
    (1.0 - 0.1_f64.powf(ratio)) as f32
}

/// peak 满程下落时长(ms)→ 每 tick 下落量(1/8 字符单位):`RES × tick / fall_ms`。
/// `fall_ms ≤ tick` 时一拍落满程(取 RES)。
///
/// # Params:
///   - `fall_ms`: 从满高([`SPECTRUM_RES`])落到 0 的满程毫秒数(配置 `peak_fall_ms`)
///   - `tick_ms`: 主循环帧间隔(毫秒)
///
/// # Return:
///   每 tick 下落量,`0.0 < v ≤ RES`。
#[allow(clippy::as_conversions)] // 纯数值换算:量级 ≤ u32::MAX,f64 内精确
fn fall_per_tick(fall_ms: u32, tick_ms: u64) -> f32 {
    let tick = tick_ms.max(1);
    if u64::from(fall_ms) <= tick {
        return f32::from(SPECTRUM_RES);
    }
    let v = f64::from(SPECTRUM_RES) * (tick as f64) / f64::from(fall_ms);
    v as f32
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
        let endpoints =
            state
                .color
                .column_endpoints(col, render_count, hue, theme, state.color_params());
        let palette_lo = endpoints.bottom;
        let palette_hi = endpoints.top;
        let bar = state.bar_at(col);
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
            } else if *state.cfg.show_trail()
                && row_from_bottom > bar_top_row
                && row_from_bottom < peak_row
            {
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
        if *state.cfg.show_peak_cap() && peak_row > bar_top_row && peak_row < area.height {
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

    use super::{
        ColorParams, SNAPSHOT_SAMPLES, SpectrumColor, SpectrumState, alpha_from_t90, fall_per_tick,
    };
    use crate::render::palette::{CoverPalette, Rgb};
    use crate::render::theme::Theme;

    /// 测试用主循环帧间隔(= default.lua 的 `animation.frame_tick_ms`)。
    const TICK_MS: u64 = 16;

    /// 纯函数测试([`cp`] / `Transition` 端点)用的任意固定过渡拍数,**不**追 default.lua;
    /// 走真实 `SpectrumState` 的测试一律读 `state.timing.fade_ticks`,与默认值解耦。
    const CP_FADE_TICKS: u32 = 300;

    /// `alpha_from_t90`:定义性质(t90/tick 拍后残差 10%)、瞬时退化、单调性。
    #[test]
    #[allow(clippy::as_conversions)]
    fn alpha_from_t90_definition_and_degenerate() {
        // 定义性质:α(t90, tick) 满足 (1-α)^(t90/tick) ≈ 0.1。
        for (t90, tick) in [(30_u32, 16_u64), (100, 16), (200, 16), (160, 16)] {
            let a = f64::from(alpha_from_t90(t90, tick));
            let residual = (1.0 - a).powf(f64::from(t90) / (tick as f64));
            assert!(
                (residual - 0.1).abs() < 1e-3,
                "t90={t90} tick={tick}: 残差应 ≈0.1,得 {residual}"
            );
        }
        // t90 ≤ tick → 瞬时。
        assert!((alpha_from_t90(16, 16) - 1.0).abs() < f32::EPSILON);
        assert!((alpha_from_t90(1, 16) - 1.0).abs() < f32::EPSILON);
        // 单调:t90 越大 α 越小(越慢)。
        assert!(alpha_from_t90(30, 16) > alpha_from_t90(100, 16));
        assert!(alpha_from_t90(100, 16) > alpha_from_t90(200, 16));
    }

    /// `fall_per_tick`:默认值精确换算(512ms→2.0/拍)、`fall_ms ≤ tick` 一拍落满。
    #[test]
    fn fall_per_tick_exact_and_degenerate() {
        assert!(
            (fall_per_tick(512, TICK_MS) - 2.0).abs() < 1e-6,
            "512ms 满程 @16ms/拍 = 2.0/拍"
        );
        assert!(
            (fall_per_tick(0, TICK_MS) - f32::from(super::SPECTRUM_RES)).abs() < f32::EPSILON,
            "0ms 一拍落满程"
        );
    }

    /// 以 defaults 配置构造频谱态(帧间隔 = [`TICK_MS`])。
    fn spectrum_state() -> color_eyre::Result<SpectrumState> {
        Ok(SpectrumState::new(
            mineral_config::Config::defaults()?.tui().spectrum().clone(),
            TICK_MS,
        ))
    }

    /// 不对称包络方向性:上升走 attack(30ms,快)、播放中下落走 decay(100ms,慢)。
    /// 同量级距离一拍的幅度,上升应明显大于下落 —— 锁住"快攻慢放"。
    #[test]
    fn envelope_attack_faster_than_decay() -> color_eyre::Result<()> {
        let mut s = spectrum_state()?;
        let n = s.bars.len();
        let high = vec![64_u16; n];
        let low = vec![0_u16; n];
        let before_rise = s.bars.first().copied().unwrap_or(0.0);
        s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&high));
        let rise = s.bars.first().copied().unwrap_or(0.0) - before_rise;
        // 推到顶附近,再喂低目标一拍取下落幅度。
        for _ in 0..50 {
            s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&high));
        }
        let before_fall = s.bars.first().copied().unwrap_or(0.0);
        s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&low));
        let fall = before_fall - s.bars.first().copied().unwrap_or(0.0);
        assert!(
            rise > fall * 1.5,
            "attack(30ms)应明显快于 decay(100ms): rise={rise} fall={fall}"
        );
        Ok(())
    }

    /// 释音慢于衰减:暂停(release 200ms)一拍的下落幅度应小于播放中向 0 目标
    /// (decay 100ms)一拍的下落幅度 —— 锁住三系数各接各的旋钮。
    #[test]
    fn envelope_release_slower_than_decay() -> color_eyre::Result<()> {
        let mut s = spectrum_state()?;
        let n = s.bars.len();
        let high = vec![64_u16; n];
        let low = vec![0_u16; n];
        for _ in 0..50 {
            s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&high));
        }
        let before = s.bars.first().copied().unwrap_or(0.0);
        s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&low));
        let fall_decay = before - s.bars.first().copied().unwrap_or(0.0);

        let mut s2 = spectrum_state()?;
        for _ in 0..50 {
            s2.tick(true /*playing*/, 100 /*volume_pct*/, Some(&high));
        }
        let before2 = s2.bars.first().copied().unwrap_or(0.0);
        s2.tick(false /*playing*/, 100 /*volume_pct*/, None);
        let fall_release = before2 - s2.bars.first().copied().unwrap_or(0.0);
        assert!(
            fall_release < fall_decay,
            "release(200ms)一拍 {fall_release} 应慢于 decay(100ms)一拍 {fall_decay}"
        );
        Ok(())
    }

    /// f32 包络收敛精确:持续喂同一目标,条高收敛到精确目标值。
    /// 旧整数定点 `(b·old+t·new)/(old+new)` 因整除截断卡在 ~95% 平台,锁住不回退。
    #[test]
    fn envelope_converges_to_exact_target() -> color_eyre::Result<()> {
        let mut s = spectrum_state()?;
        let n = s.bars.len();
        let high = vec![64_u16; n];
        for _ in 0..100 {
            s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&high));
        }
        let b = s.bars.first().copied().unwrap_or(0.0);
        assert!((b - 64.0).abs() < 0.5, "应收敛到精确目标 64,得 {b}");
        Ok(())
    }

    /// 释音收敛:暂停后条高滑向 0、由 baseline(3)兜底停住,面板不死寂。
    #[test]
    fn release_settles_at_baseline() -> color_eyre::Result<()> {
        let mut s = spectrum_state()?;
        let n = s.bars.len();
        let high = vec![64_u16; n];
        for _ in 0..50 {
            s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&high));
        }
        for _ in 0..200 {
            s.tick(false /*playing*/, 100 /*volume_pct*/, None);
        }
        let b = s.bars.first().copied().unwrap_or(0.0);
        assert!(
            (b - 3.0).abs() < f32::EPSILON,
            "释音后应停在 baseline=3,得 {b}"
        );
        Ok(())
    }

    /// 色场计算参数(任意固定值,纯函数测试用,不依赖 default.lua)。
    fn cp() -> ColorParams {
        ColorParams {
            vshift_permille: 200,
            fade_ticks: CP_FADE_TICKS,
        }
    }

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
        let state = spectrum_state()?;
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
        let mut state = spectrum_state()?;
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
        let mut state = spectrum_state()?;
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

    /// `begin_cover_transition` 后 tick 过渡拍数次,落到 `CoverFixed`(静止)。
    #[test]
    fn cover_transition_settles_to_fixed() -> color_eyre::Result<()> {
        let mut s = spectrum_state()?;
        s.begin_cover_transition(fixed_palette()?, &Theme::default());
        assert!(
            matches!(s.color, SpectrumColor::Transition { .. }),
            "begin 后应进 Transition"
        );
        for _ in 0..s.timing.fade_ticks {
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
        let mut s = spectrum_state()?;
        s.begin_cover_transition(fixed_palette()?, &Theme::default());
        s.clear_cover();
        assert!(matches!(s.color, SpectrumColor::Hue), "clear 后应回 Hue");
        Ok(())
    }

    /// `Hue` 态 tick 推进 `hue_phase`;`CoverFixed` 态 tick 不改色(端点两帧一致)。
    #[test]
    fn hue_advances_but_coverfixed_is_frozen() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let mut s = spectrum_state()?;
        let before = s.hue_phase;
        s.tick(false /*playing*/, 100 /*volume_pct*/, None);
        assert_ne!(s.hue_phase, before, "Hue 态 hue_phase 应自增");

        s.begin_cover_transition(fixed_palette()?, &theme);
        for _ in 0..s.timing.fade_ticks {
            s.tick(false /*playing*/, 100 /*volume_pct*/, None);
        }
        let ep1 = s.color.column_endpoints(2, 16, s.hue_deg(), &theme, cp());
        s.tick(false /*playing*/, 100 /*volume_pct*/, None);
        let ep2 = s.color.column_endpoints(2, 16, s.hue_deg(), &theme, cp());
        assert_eq!(ep1, ep2, "CoverFixed 态 tick 不应改色");
        Ok(())
    }

    /// `Hue` 态:所有列端点相同(与旧单色实现等价,零回归)。
    #[test]
    fn hue_columns_are_uniform() {
        let theme = Theme::default();
        let color = SpectrumColor::Hue;
        let first = color.column_endpoints(
            /*col*/ 0,
            /*bar_count*/ 16,
            /*hue*/ 30.0,
            &theme,
            cp(),
        );
        let mid = color.column_endpoints(
            /*col*/ 8,
            /*bar_count*/ 16,
            /*hue*/ 30.0,
            &theme,
            cp(),
        );
        let last = color.column_endpoints(
            /*col*/ 15,
            /*bar_count*/ 16,
            /*hue*/ 30.0,
            &theme,
            cp(),
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
            /*col*/ 0,
            /*bar_count*/ 16,
            /*hue*/ 0.0,
            &theme,
            cp(),
        );
        let last = color.column_endpoints(
            /*col*/ 15,
            /*bar_count*/ 16,
            /*hue*/ 0.0,
            &theme,
            cp(),
        );
        assert_ne!(first.bottom, last.bottom, "底色应沿频率轴变化");
        Ok(())
    }

    /// `Transition`(从 `Hue` 起步)态:`frame=0` 等于 frozen-hue 起点,`frame=CP_FADE_TICKS`
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
        let hue_start = SpectrumColor::Hue.column_endpoints(
            /*col*/ 3,
            /*bar_count*/ 16,
            frozen,
            &theme,
            cp(),
        );
        assert_eq!(
            at_start.column_endpoints(
                /*col*/ 3,
                /*bar_count*/ 16,
                /*hue*/ 999.0,
                &theme,
                cp()
            ),
            hue_start,
            "progress=0 应等于 frozen hue 端点"
        );

        let at_end = SpectrumColor::Transition {
            from: Box::new(SpectrumColor::Hue),
            frozen_hue_deg: frozen,
            to: palette.clone(),
            frame: CP_FADE_TICKS,
        };
        let fixed = SpectrumColor::CoverFixed { palette };
        assert_eq!(
            at_end.column_endpoints(
                /*col*/ 3,
                /*bar_count*/ 16,
                /*hue*/ 0.0,
                &theme,
                cp()
            ),
            fixed.column_endpoints(
                /*col*/ 3,
                /*bar_count*/ 16,
                /*hue*/ 0.0,
                &theme,
                cp()
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
        let mut s = spectrum_state()?;
        // 先进入红色场静止。
        s.begin_cover_transition(red.clone(), &theme);
        for _ in 0..s.timing.fade_ticks {
            s.tick(false /*playing*/, 100 /*volume_pct*/, None);
        }
        assert!(
            matches!(s.color, SpectrumColor::CoverFixed { .. }),
            "应先静止在红"
        );

        // 换蓝:刚切(frame=0)那刻起点端点应等于红色场,而不是 hue。
        s.begin_cover_transition(blue, &theme);
        let at_start = s.color.column_endpoints(
            /*col*/ 3,
            /*bar_count*/ 16,
            s.hue_deg(),
            &theme,
            cp(),
        );
        let red_endpoints = SpectrumColor::CoverFixed { palette: red }.column_endpoints(
            /*col*/ 3,
            /*bar_count*/ 16,
            /*hue*/ 0.0,
            &theme,
            cp(),
        );
        assert_eq!(
            at_start, red_endpoints,
            "红→蓝起点应是红色场,不是 hue 初始色"
        );
        Ok(())
    }

    /// 打断回归:红→蓝过渡走到一半再换目标,新过渡起点 = 打断那刻的可见中间色
    /// (烘焙快照),而非跳回打断前的目标(蓝)。`bar_count` 取 [`SNAPSHOT_SAMPLES`]
    /// 让列位置正落在采样网格上,端点可逐列**精确**比较(无插值弦差)。
    #[test]
    fn interrupting_transition_starts_from_visible_mix() -> color_eyre::Result<()> {
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
        let mut s = spectrum_state()?;
        // 静止在红,再向蓝过渡到半程:可见色 = 红蓝中间色。
        s.begin_cover_transition(red.clone(), &theme);
        for _ in 0..s.timing.fade_ticks {
            s.tick(false /*playing*/, 100 /*volume_pct*/, None);
        }
        s.begin_cover_transition(blue.clone(), &theme);
        for _ in 0..s.timing.fade_ticks / 2 {
            s.tick(false /*playing*/, 100 /*volume_pct*/, None);
        }
        // 采样真实 state 的中途 Transition 端点必须用 state 自己的色场参数
        // (进度分母 = timing.fade_ticks),与 freeze 烘焙的口径一致;用 cp() 会失谐。
        let p = s.color_params();
        let visible = (0..SNAPSHOT_SAMPLES)
            .map(|col| {
                s.color
                    .column_endpoints(col, SNAPSHOT_SAMPLES, s.hue_deg(), &theme, p)
            })
            .collect::<Vec<_>>();
        // 半程中间色应不同于打断目标(蓝),否则下面的连续性断言空洞无意义。
        let blue_first = SpectrumColor::CoverFixed { palette: blue }.column_endpoints(
            /*col*/ 0,
            SNAPSHOT_SAMPLES,
            /*hue*/ 0.0,
            &theme,
            p,
        );
        let visible_first = visible
            .first()
            .copied()
            .ok_or_else(|| color_eyre::eyre::eyre!("采样非空"))?;
        assert_ne!(visible_first, blue_first, "半程中间色应不同于打断目标");

        // 半程打断、换回红:起点应是烘焙快照,frame=0 端点逐列等于打断前可见色(不跳变)。
        s.begin_cover_transition(red, &theme);
        assert!(
            matches!(
                &s.color,
                SpectrumColor::Transition { from, .. }
                    if matches!(from.as_ref(), SpectrumColor::Snapshot { .. })
            ),
            "打断后的过渡起点应是烘焙快照"
        );
        for (col, want) in visible.iter().enumerate() {
            let got = s
                .color
                .column_endpoints(col, SNAPSHOT_SAMPLES, s.hue_deg(), &theme, p);
            assert_eq!(got, *want, "col={col} 打断后起点应与打断前可见色连续");
        }
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
        let mut state = spectrum_state()?;
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
        state.begin_cover_transition(fixed_palette()?, &theme);
        for _ in 0..state.timing.fade_ticks {
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
