//! 频谱运行态:FFT 真值条的 ADSR 包络 + peak hold/弹簧 + 配色态机 + baseline 兜底。
//!
//! 数据由 [`mineral_spectrum::SpectrumComputer`] 算出目标条高,
//! [`SpectrumState::tick`] 按效果器 ADSR 包络写入:attack(上升)/ decay(播放中
//! 余韵滑落)/ release(暂停释音落 0),sustain 即 FFT 实时值。时长旋钮均为毫秒,
//! 构造时按 `animation.frame_tick_ms` 折算成每拍系数,与帧率解耦。装饰两件:
//!
//! 1. **Peak cap**:每根条记一个 peak,瞬间跟涨,顶部 hold 一段时间再缓慢下落。
//!    渲染为浅色 ▔ 横线浮在条顶上方一格,经典 KTV / Winamp 风格。
//! 2. **Baseline**:任何状态下条高都不低于配置的 `baseline_min`,面板永远不死寂。
//!    pause 时条衰减到 baseline 停住,FFT 还没出第一窗时也是 baseline。

use std::cell::Cell;
use std::collections::VecDeque;

use mineral_config::SpectrumStyle;
use ratatui::style::Color;

use crate::render::anim::{ticks16_from_ms, ticks32_from_ms};
use crate::render::color::{lerp_color, rotate_hue};
use crate::render::palette::{ColumnColors, CoverPalette, Rgb, column_permille};
use crate::render::theme::Theme;

/// 频谱柱条的逻辑分辨率(每格 1/8 字符高度,共 8 行 × 8 = 64 单位)。
pub(super) const SPECTRUM_RES: u16 = mineral_spectrum::RES;

/// 首帧 / 重启时的默认条数。首帧渲染后被实际 area.width 推算的值覆盖。
const DEFAULT_BAR_COUNT: usize = 64;

/// scope 包络历史环容量上限(列)。渲染按面板点宽取尾部,上限只防无界增长
/// (16ms/列 × 2048 ≈ 33s,远超任何面板宽度所需;整环 16KB)。
const SCOPE_HIST_CAP: usize = 2048;

/// waterfall 历史环容量上限(行)。渲染按面板行数取所需前缀,上限只防无界增长
/// (64ms/行 × 256 ≈ 16s,远超任何面板高度所需;行宽 ~200 列时整环 ≈ 100KB)。
const WATER_HIST_CAP: usize = 256;

/// scope 单列时域 min/max 包络(已乘音量,-1..=1 量级)。
/// 一列 = `scope.column_ms` 毫秒音频的幅度极值,推入历史环后不再改写。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct WaveSpan {
    /// 列内最小采样值。
    pub(super) min: f32,

    /// 列内最大采样值。
    pub(super) max: f32,
}

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

    /// waterfall 推行间隔拍数,来自 `waterfall.push_ms`。
    water_push_ticks: u16,

    /// terrain 推层间隔拍数,来自 `terrain.push_ms`。
    terrain_push_ticks: u16,
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
            peak_hold_ticks: ticks16_from_ms(*cfg.bars().peak_hold_ms(), tick_ms),
            peak_fall_per_tick: fall_per_tick(*cfg.bars().peak_fall_ms(), tick_ms),
            hue_cycle_ticks: ticks32_from_ms(*cfg.hue_cycle_ms(), tick_ms),
            fade_ticks: ticks32_from_ms(*cfg.cover_fade_ms(), tick_ms),
            water_push_ticks: ticks16_from_ms(*cfg.waterfall().push_ms(), tick_ms).max(1),
            terrain_push_ticks: ticks16_from_ms(*cfg.terrain().push_ms(), tick_ms).max(1),
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
    /// `Cell` 是因为渲染层只拿 `&SpectrumState`,这是「render → tick」反向通道。
    pub target_bars: Cell<usize>,

    /// waterfall 历史环(头部最新)。行 = 推行那刻的 FFT 真值条(已乘音量,
    /// 不过 ADSR——瀑布要锐利的瞬时值,余韵是历史本身)。仅 `style = waterfall`
    /// 时推进;行按推行那刻的列数存,渲染按当前面板宽插值读取——列数变化
    /// (resize / browse↔fullscreen 切换)**不清环**,否则每次切布局画面清空重攒。
    water_hist: VecDeque<Box<[u16]>>,

    /// 距下次 waterfall 推行的剩余拍数。
    water_countdown: u16,

    /// terrain 历史层(头部最新)。层 = 推层那刻 ADSR 平滑后的条高快照
    /// (f32,0..=RES)——decay 余韵让相邻层时间连贯,喂真值山脊会碎成条纹。
    /// 仅 `style = terrain` 时推进;渲染按层长插值读取,层长与面板宽解耦。
    terrain_hist: VecDeque<Box<[f32]>>,

    /// 距下次 terrain 推层的剩余拍数。渲染经 [`Self::terrain_progress`] 读它做
    /// 层间滚动插值——地形连续上浮而非整层跳变。
    terrain_countdown: u16,

    /// scope 包络历史环(尾部最新,渲染右新左旧)。列 = `scope.column_ms` 毫秒
    /// 音频的 min/max 极值(已乘音量);无新样本(暂停)时冻结不动。
    /// 仅 `style = scope` 时经 [`Self::tick_scope`] 更新。
    wave: VecDeque<WaveSpan>,

    /// scope 聚合进行中的余样本(不足一列的尾巴,下批样本续上)。
    wave_carry: Vec<f32>,

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
            water_hist: VecDeque::new(),
            water_countdown: 0,
            terrain_hist: VecDeque::new(),
            terrain_countdown: 0,
            wave: VecDeque::new(),
            wave_carry: Vec::new(),
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
    pub(super) fn spring_peak_at(&self, col: usize) -> u16 {
        let raw = self.peak_pos.get(col).copied().unwrap_or(0.0);
        let clamped = raw.clamp(0.0, f32::from(SPECTRUM_RES));
        clamped.round() as u16
    }

    /// `col` 列的条高收整(渲染用):clamp 到 `0..=RES` 再 round 成 u16。
    /// 内部包络是 f32(收敛精确、无整数截断),只在渲染口收整。
    #[allow(clippy::as_conversions)]
    pub(super) fn bar_at(&self, col: usize) -> u16 {
        let raw = self.bars.get(col).copied().unwrap_or(0.0);
        let clamped = raw.clamp(0.0, f32::from(SPECTRUM_RES));
        clamped.round() as u16
    }

    /// 渲染侧读:条状态数组长度。resize 间隙可能与面板宽不一致,渲染按 min 取。
    pub(super) fn bar_len(&self) -> usize {
        self.bars.len()
    }

    /// 渲染侧读:频谱旋钮(观感开关 / 风格)。
    pub(super) fn cfg(&self) -> &mineral_config::SpectrumConfig {
        &self.cfg
    }

    /// 第 `col` 列(共 `bar_count` 列)的底/顶端点色。hue 相位与色场参数由内部提供,
    /// 配色态机细节不出本模块。
    pub(super) fn column_colors(
        &self,
        col: usize,
        bar_count: usize,
        theme: &Theme,
    ) -> ColumnColors {
        self.color
            .column_endpoints(col, bar_count, self.hue_deg(), theme, self.color_params())
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
    ///
    /// **例外**:waterfall / terrain 暂停时整体冻结(历史环 + 当前轮廓都静止,
    /// 暂停就是要停下来观察历史频段),只有配色环境继续走;释音塌线是 bars 专属。
    pub fn tick(&mut self, playing: bool, volume_pct: u8, bars: Option<&[u16]>) {
        let style = *self.cfg.style();
        if !playing && matches!(style, SpectrumStyle::Waterfall | SpectrumStyle::Terrain) {
            self.advance_color();
            return;
        }
        if style == SpectrumStyle::Waterfall {
            self.push_water(bars, volume_pct);
        }
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
        // 放包络推进之后:terrain 层要的是本拍平滑结果的快照。
        if *self.cfg.style() == SpectrumStyle::Terrain {
            self.push_terrain();
        }
    }

    /// scope 专用 tick:把本拍新到的 PCM 样本聚合进包络历史环并推进配色。
    /// 与条形家族的 [`Self::tick`] 互斥使用(消费端按 style 分路,一帧只走一个入口)。
    ///
    /// 每满 `scope.column_ms` 毫秒音频(按 `sample_rate` 折算的样本数)出一根
    /// min/max 列推入环尾;不足一列的尾巴留在 carry 等下批。列与**音频时间**对齐
    /// 而非渲染帧——样本到达节奏抖动不影响滚动速度,这是波形不左右晃的关键。
    ///
    /// 无新样本(暂停 / 起播间隙)时画面**冻结**(DAW 语义:暂停就是要停下来
    /// 观察波形),恢复播放从暂停点续推;不做释音塌线,故不需要 `playing`。
    ///
    /// # Params:
    ///   - `volume_pct`: 音量百分比(听感联动:包络乘 `vol/100`)
    ///   - `samples`: 本拍拉到的 PCM 样本(可空:起播间隙 / 暂停)
    ///   - `sample_rate`: PCM 采样率(Hz);0(未知)时按每样本一列退化处理
    pub fn tick_scope(&mut self, volume_pct: u8, samples: &[f32], sample_rate: u32) {
        if samples.is_empty() {
            self.advance_color();
            return;
        }
        let per_column = self.scope_samples_per_column(sample_rate);
        let vol = f32::from(volume_pct.min(100)) / 100.0;
        self.wave_carry.extend_from_slice(samples);
        let complete = self.wave_carry.len() / per_column;
        for chunk in self.wave_carry.chunks_exact(per_column) {
            let (min, max) = chunk
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), s| {
                    (lo.min(*s), hi.max(*s))
                });
            self.wave.push_back(WaveSpan {
                min: min * vol,
                max: max * vol,
            });
        }
        self.wave_carry.drain(..complete * per_column);
        while self.wave.len() > SCOPE_HIST_CAP {
            self.wave.pop_front();
        }
        self.advance_color();
    }

    /// scope 一列聚合的样本数(`scope.column_ms` 按采样率折算,至少 1)。
    fn scope_samples_per_column(&self, sample_rate: u32) -> usize {
        let column_ms = u64::from(*self.cfg.scope().column_ms()).max(1);
        let per = u64::from(sample_rate) * column_ms / 1000;
        usize::try_from(per).unwrap_or(usize::MAX).max(1)
    }

    /// 渲染侧读:从最新往回数第 `idx` 根包络列(0 = 最新,渲染贴右缘)。
    /// 超出历史返回 `None`(面板比历史宽的左侧空白,渲染画中线)。
    pub(super) fn wave_span_from_newest(&self, idx: usize) -> Option<WaveSpan> {
        let i = self.wave.len().checked_sub(1 + idx)?;
        self.wave.get(i).copied()
    }

    /// waterfall 历史推行:按 `waterfall.push_ms` 折算的节奏把当刻 FFT 真值
    /// (乘音量,不过 ADSR)推入历史环头部。起播间隙(在播但 FFT 窗未满)推
    /// 静默行(时间轴上那一刻确实无声);暂停不会走到这里([`Self::tick`] 冻结)。
    /// 行长 = 推行那刻的列数,渲染按当前面板宽插值读取(与 terrain 同策略),
    /// 列数变化不清环。
    fn push_water(&mut self, bars: Option<&[u16]>, volume_pct: u8) {
        if self.water_countdown > 0 {
            self.water_countdown -= 1;
            return;
        }
        self.water_countdown = self.timing.water_push_ticks.saturating_sub(1);
        let cols = self.target_bars.get().max(1);
        let mut row = vec![0_u16; cols].into_boxed_slice();
        if let Some(targets) = bars {
            let vol = u32::from(volume_pct.min(100));
            for (slot, target) in row.iter_mut().zip(targets.iter()) {
                *slot = u16::try_from(u32::from(*target) * vol / 100).unwrap_or(0);
            }
        }
        self.water_hist.push_front(row);
        self.water_hist.truncate(WATER_HIST_CAP);
    }

    /// 渲染侧读:第 `idx` 行 waterfall 历史(0 = 最新)。超出历史返回 `None`。
    pub(super) fn water_row(&self, idx: usize) -> Option<&[u16]> {
        self.water_hist.get(idx).map(AsRef::as_ref)
    }

    /// terrain 推层:按 `terrain.push_ms` 折算的节奏快照当前平滑条高。
    fn push_terrain(&mut self) {
        if self.terrain_countdown > 0 {
            self.terrain_countdown -= 1;
            return;
        }
        self.terrain_countdown = self.timing.terrain_push_ticks.saturating_sub(1);
        self.terrain_hist
            .push_front(self.bars.clone().into_boxed_slice());
        self.terrain_hist
            .truncate((*self.cfg.terrain().layers()).max(1));
    }

    /// 渲染侧读:第 `idx` 层 terrain 历史(0 = 最新最前)。超出层数返回 `None`。
    pub(super) fn terrain_layer(&self, idx: usize) -> Option<&[f32]> {
        self.terrain_hist.get(idx).map(AsRef::as_ref)
    }

    /// 渲染侧读:距上次推层的进度(0..1)。渲染用它把所有历史层连续上浮
    /// `progress × 层距`——推层瞬间新层从最前山脊位置无缝接棒,地形匀速滚动
    /// 而非每 `terrain.push_ms` 整层跳一格。
    pub(super) fn terrain_progress(&self) -> f32 {
        let ticks = self.timing.terrain_push_ticks.max(1);
        let elapsed = ticks
            .saturating_sub(1)
            .saturating_sub(self.terrain_countdown);
        f32::from(elapsed) / f32::from(ticks)
    }

    /// 渲染侧读:当前 ADSR 平滑条高(f32 真值,0..=RES)。terrain 拿它画最前山脊
    /// (固定在底部的「现在」),历史层从这里起浮。
    pub(super) fn smoothed_bars(&self) -> &[f32] {
        &self.bars
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
        if !*self.cfg.bars().spring_peak() {
            for (pos, p) in self.peak_pos.iter_mut().zip(self.peaks.iter()) {
                *pos = *p;
            }
            return;
        }
        let (stiffness, damping) = (
            *self.cfg.bars().spring_stiffness(),
            *self.cfg.bars().spring_damping(),
        );
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

    /// 以「defaults + overlay」合成配置构造频谱态(与 daemon 合成路径同构),
    /// 用于覆盖 `style` 及 per-style 子表旋钮。
    fn spectrum_state_with(overlay: serde_json::Value) -> color_eyre::Result<SpectrumState> {
        let tree = mineral_config::merge_tree(mineral_config::default_tree()?, overlay);
        let cfg = mineral_config::from_tree(&tree)
            .map_err(|w| color_eyre::eyre::eyre!("overlay 落型失败: {w}"))?;
        Ok(SpectrumState::new(cfg.tui().spectrum().clone(), TICK_MS))
    }

    /// bars 风格不推 waterfall / terrain 历史(不为用不上的画面攒内存)。
    #[test]
    fn bars_style_keeps_histories_empty() -> color_eyre::Result<()> {
        let mut s = spectrum_state()?;
        let n = s.target_bars.get();
        let bars = vec![40_u16; n];
        for _ in 0..32 {
            s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        assert!(s.water_hist.is_empty(), "bars 风格不该推 waterfall 历史");
        assert!(s.terrain_hist.is_empty(), "bars 风格不该推 terrain 历史");
        Ok(())
    }

    /// style = waterfall 的频谱态。
    fn waterfall_state() -> color_eyre::Result<SpectrumState> {
        spectrum_state_with(serde_json::json!({ "tui": { "spectrum": {
            "style": "waterfall",
        } } }))
    }

    /// waterfall:推行按节奏走(默认 64ms/行 @16ms = 每 4 拍一行),行内容是 FFT 真值×音量。
    #[test]
    fn waterfall_pushes_rows_on_cadence() -> color_eyre::Result<()> {
        let mut s = waterfall_state()?;
        let n = s.target_bars.get();
        let bars = vec![40_u16; n];
        assert!(s.water_hist.is_empty(), "初始无历史");
        for _ in 0..16 {
            s.tick(true /*playing*/, 50 /*volume_pct*/, Some(&bars));
        }
        assert_eq!(s.water_hist.len(), 4, "16 拍 @4拍/行 应推 4 行");
        let newest = s
            .water_hist
            .front()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有最新行"))?;
        assert_eq!(newest.first().copied(), Some(20), "真值 40 × 音量 50% = 20");
        Ok(())
    }

    /// waterfall:暂停整幅**冻结**(历史 + 当前行都静止,停下观察历史频段),
    /// 不推静默行流走;即便暂停期 FFT 仍给旧窗真值也不推。
    #[test]
    fn waterfall_pause_freezes_history() -> color_eyre::Result<()> {
        let mut s = waterfall_state()?;
        let n = s.target_bars.get();
        let bars = vec![40_u16; n];
        for _ in 0..8 {
            s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        let before = s.water_hist.clone();
        assert!(!before.is_empty(), "前置:已有历史");
        for _ in 0..8 {
            s.tick(false /*playing*/, 100 /*volume_pct*/, None);
            // 暂停期 FFT 环形窗还留着旧样本,可能继续给出真值——同样冻结。
            s.tick(false /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        assert_eq!(s.water_hist, before, "暂停期历史环应逐行冻结不动");
        Ok(())
    }

    /// terrain:暂停整幅冻结——不推层、最前轮廓(平滑条)与推层进度都静止。
    #[test]
    fn terrain_pause_freezes_layers() -> color_eyre::Result<()> {
        let mut s = spectrum_state_with(serde_json::json!({ "tui": { "spectrum": {
            "style": "terrain",
        } } }))?;
        let n = s.target_bars.get();
        let bars = vec![40_u16; n];
        for _ in 0..32 {
            s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        let hist_before = s.terrain_hist.clone();
        let bars_before = s.bars.clone();
        let progress_before = s.terrain_progress();
        for _ in 0..32 {
            s.tick(false /*playing*/, 100 /*volume_pct*/, None);
            s.tick(false /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        assert_eq!(s.terrain_hist, hist_before, "暂停期不该推层");
        assert_eq!(s.bars, bars_before, "暂停期最前轮廓(平滑条)应静止");
        assert!(
            (s.terrain_progress() - progress_before).abs() < f32::EPSILON,
            "暂停期推层进度应静止(山脊不上浮)"
        );
        Ok(())
    }

    /// waterfall:列数变化(resize / browse↔fullscreen 切换)**不清环**——
    /// 旧行按原长保留,渲染插值读取;清环会让每次切布局画面清空重攒。
    #[test]
    fn waterfall_resize_keeps_history() -> color_eyre::Result<()> {
        let mut s = waterfall_state()?;
        let n = s.target_bars.get();
        let bars = vec![40_u16; n];
        for _ in 0..8 {
            s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        let rows_before = s.water_hist.len();
        assert!(rows_before > 0, "前置:已有历史");
        s.target_bars.set(n + 7);
        let wider = vec![40_u16; n + 7];
        for _ in 0..4 {
            s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&wider));
        }
        assert!(
            s.water_hist.len() > rows_before,
            "变宽后旧历史应保留,新行继续叠加"
        );
        assert!(
            s.water_hist.iter().any(|row| row.len() == n),
            "旧行按原长保留(渲染插值读取,不重采样存储)"
        );
        Ok(())
    }

    /// waterfall 跨宽度渲染:窄面板攒的历史在宽面板(fullscreen 通栏)照常显示,
    /// 顶行有内容——锁住「切布局不清空」的插值读取。
    #[test]
    fn waterfall_renders_history_across_width_change() -> color_eyre::Result<()> {
        let mut narrow = Terminal::new(TestBackend::new(40, 10))?;
        let mut state = waterfall_state()?;
        narrow.draw(|f| super::super::draw(f, f.area(), &state, &Theme::default()))?;
        let bars = jagged_bars(state.target_bars.get());
        for _ in 0..40 {
            state.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        // 切到更宽的面板(模拟 fullscreen 通栏),历史行长(38)< 新内宽(78)。
        let mut wide = Terminal::new(TestBackend::new(80, 10))?;
        wide.draw(|f| super::super::draw(f, f.area(), &state, &Theme::default()))?;
        let buf = wide.backend().buffer();
        let has_glyph = (1..79_u16).any(|x| {
            buf.cell((x, 1))
                .is_some_and(|cell| cell.symbol() == "▀" || cell.symbol() == "▄")
        });
        assert!(has_glyph, "宽面板顶行应插值显示旧历史,而非清空");
        Ok(())
    }

    /// waterfall 稳态快照:▀ 热力半块,一字符行装两帧历史,最新在顶。
    #[test]
    fn waterfall_heat_snapshot() -> color_eyre::Result<()> {
        let mut terminal = Terminal::new(TestBackend::new(80, 10))?;
        let mut state = waterfall_state()?;
        terminal.draw(|f| super::super::draw(f, f.area(), &state, &Theme::default()))?;
        let bars = jagged_bars(state.target_bars.get());
        for _ in 0..80 {
            state.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        terminal.draw(|f| super::super::draw(f, f.area(), &state, &Theme::default()))?;
        crate::test_support::assert_snap!(
            "waterfall 热力半块稳态(历史 20 帧,▀ 铺满上部)",
            terminal.backend()
        );
        Ok(())
    }

    /// heat 半块配色:上下两帧幅度不同的格,fg ≠ bg 且字形为 ▀
    /// (锁住「一格装两帧」的双色接线)。
    #[test]
    fn waterfall_heat_cell_packs_two_frames() -> color_eyre::Result<()> {
        let mut terminal = Terminal::new(TestBackend::new(80, 10))?;
        let mut state = waterfall_state()?;
        terminal.draw(|f| super::super::draw(f, f.area(), &state, &Theme::default()))?;
        let n = state.target_bars.get();
        // 幅度随 tick 递增:相邻两帧历史必然不同 → 顶行格 fg ≠ bg。
        for step in 0..80_u16 {
            let bars = vec![(step % 60) + 4; n];
            state.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        terminal.draw(|f| super::super::draw(f, f.area(), &state, &Theme::default()))?;
        let buf = terminal.backend().buffer();
        let cell = buf
            .cell((40, 1)) // 面板内区顶行中列
            .ok_or_else(|| color_eyre::eyre::eyre!("cell 缺失"))?;
        assert_eq!(cell.symbol(), "▀", "heat 格应是上半块");
        assert_ne!(cell.fg, cell.bg, "上下两帧幅度不同 → fg ≠ bg");
        Ok(())
    }

    /// terrain:推层按节奏走(128ms @16ms = 每 8 拍一层),层内容是 ADSR 平滑
    /// 快照——首层在包络刚起步时推出,值应明显低于目标(锁「平滑而非真值」,
    /// 喂真值层间会时间不连贯、山脊碎成条纹)。
    #[test]
    fn terrain_pushes_smoothed_layers_on_cadence() -> color_eyre::Result<()> {
        let mut s = spectrum_state_with(serde_json::json!({ "tui": { "spectrum": {
            "style": "terrain",
        } } }))?;
        let n = s.target_bars.get();
        let bars = vec![40_u16; n];
        for _ in 0..32 {
            s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        assert_eq!(s.terrain_hist.len(), 4, "32 拍 @8拍/层 应推 4 层");
        let first_layer_value = s
            .terrain_hist
            .back()
            .and_then(|layer| layer.first())
            .copied()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有最早层"))?;
        assert!(
            first_layer_value > 3.0 && first_layer_value < 39.0,
            "首层应是包络中途的平滑值(baseline 3 与目标 40 之间),得 {first_layer_value}"
        );
        Ok(())
    }

    /// terrain:层数封顶(环容量 = `terrain.layers`),不无界增长。
    #[test]
    fn terrain_layers_capped() -> color_eyre::Result<()> {
        let mut s = spectrum_state_with(serde_json::json!({ "tui": { "spectrum": {
            "style": "terrain",
        } } }))?;
        let n = s.target_bars.get();
        let bars = vec![40_u16; n];
        for _ in 0..200 {
            s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        assert_eq!(s.terrain_hist.len(), 8, "层数应封顶在默认 8");
        Ok(())
    }

    /// terrain 推层进度:推层拍归零,层间随拍单调爬升且恒 < 1
    /// (渲染按它上浮插值,越层会让山脊瞬移)。
    #[test]
    fn terrain_progress_ramps_between_pushes() -> color_eyre::Result<()> {
        let mut s = spectrum_state_with(serde_json::json!({ "tui": { "spectrum": {
            "style": "terrain",
        } } }))?;
        let n = s.target_bars.get();
        let bars = vec![40_u16; n];
        s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars)); // 推层拍
        assert!(s.terrain_progress() < 0.01, "推层瞬间进度应归零");
        let mut last = s.terrain_progress();
        // 默认 128ms @16ms/拍 = 每 8 拍推层:中间 7 拍进度爬升。
        for i in 0..7 {
            s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
            let p = s.terrain_progress();
            assert!(p > last && p < 1.0, "第 {i} 拍进度应单调爬升且 < 1,得 {p}");
            last = p;
        }
        s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars)); // 下一推层拍
        assert!(s.terrain_progress() < 0.01, "再推层进度应再归零");
        Ok(())
    }

    /// terrain 稳态快照:Braille 山脊层叠,最新层在最下,前景遮挡后景。
    #[test]
    fn terrain_snapshot() -> color_eyre::Result<()> {
        let mut terminal = Terminal::new(TestBackend::new(80, 12))?;
        let mut state = spectrum_state_with(serde_json::json!({ "tui": { "spectrum": {
            "style": "terrain",
        } } }))?;
        terminal.draw(|f| super::super::draw(f, f.area(), &state, &Theme::default()))?;
        let n = state.target_bars.get();
        // 幅度随 tick 波动:相邻层轮廓不同,遮挡关系可见。
        for step in 0..64_u16 {
            let bars = (0..n)
                .map(|i| {
                    let base = 8 + u16::try_from((i * 13) % 40).unwrap_or(0);
                    (base + step % 16).min(64)
                })
                .collect::<Vec<u16>>();
            state.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        terminal.draw(|f| super::super::draw(f, f.area(), &state, &Theme::default()))?;
        crate::test_support::assert_snap!(
            "terrain 山脊地形稳态(最前活层 + 历史层进度上浮,画家算法遮挡)",
            terminal.backend()
        );
        Ok(())
    }

    /// scope 测试用采样率(任意固定值,只要与 `column_ms` 一起折算出的
    /// 每列样本数 > 0)。
    const SCOPE_TEST_SR: u32 = 48_000;

    /// style = scope 的频谱态。
    fn scope_state() -> color_eyre::Result<SpectrumState> {
        spectrum_state_with(serde_json::json!({ "tui": { "spectrum": {
            "style": "scope",
        } } }))
    }

    /// `s` 折算出的 scope 每列样本数([`SCOPE_TEST_SR`] 口径)。
    fn per_column(s: &SpectrumState) -> color_eyre::Result<usize> {
        let column_ms = usize::try_from(*s.cfg.scope().column_ms())?;
        Ok(usize::try_from(SCOPE_TEST_SR)? * column_ms / 1000)
    }

    /// 造恰好聚成 `column_count` 列的正弦样本:幅度从头(旧)到尾(新)线性衰减
    /// ——包络呈楔形,快照能锁住「轮廓随时间变化」而非一块实心砖。
    #[allow(clippy::as_conversions)]
    fn sine_samples(column_count: usize, samples_per_column: usize) -> Vec<f32> {
        let n = column_count * samples_per_column;
        (0..n)
            .map(|i| {
                let envelope = 1.0 - (i as f32) / (n as f32);
                (2.0 * std::f32::consts::PI * 750.0 * (i as f32) / 48_000.0).sin() * envelope
            })
            .collect::<Vec<f32>>()
    }

    /// scope:喂样本即聚合出包络列(乘音量);暂停(无新样本)后整幅**冻结**
    /// (DAW 语义,停下观察波形),不做释音塌线。
    #[test]
    fn scope_pause_freezes_wave() -> color_eyre::Result<()> {
        let mut s = scope_state()?;
        let samples = sine_samples(64, per_column(&s)?);
        s.tick_scope(100 /*volume_pct*/, &samples, SCOPE_TEST_SR);
        assert_eq!(s.wave.len(), 64, "64 列音频应恰聚出 64 列包络");
        let peak = s.wave.iter().map(|span| span.max).fold(0.0_f32, f32::max);
        assert!(peak > 0.9, "楔形头部应近满幅,得 {peak}");
        let before = s.wave.clone();
        for _ in 0..400 {
            s.tick_scope(100 /*volume_pct*/, &[], SCOPE_TEST_SR);
        }
        assert_eq!(s.wave, before, "暂停期包络应逐列冻结不动");
        Ok(())
    }

    /// scope:音量缩放包络(50% 音量 → 幅度减半)。
    #[test]
    fn scope_wave_scales_with_volume() -> color_eyre::Result<()> {
        let mut s = scope_state()?;
        let samples = sine_samples(64, per_column(&s)?);
        s.tick_scope(50 /*volume_pct*/, &samples, SCOPE_TEST_SR);
        let peak = s.wave.iter().map(|span| span.max).fold(0.0_f32, f32::max);
        assert!(
            (0.4..=0.55).contains(&peak),
            "50% 音量满幅正弦应近半幅,得 {peak}"
        );
        Ok(())
    }

    /// scope 滚动时间序:先安静后响两批样本,环尾(最新,渲染贴右缘)是响的、
    /// 环头是安静的;不足一列的尾巴留在 carry 不出列。
    #[test]
    fn scope_scroll_keeps_time_order() -> color_eyre::Result<()> {
        let mut s = scope_state()?;
        let per = per_column(&s)?;
        let quiet = vec![0.2_f32; per * 2];
        // 响批多带半列尾巴:验证 carry 只攒不出列。
        let loud = vec![1.0_f32; per * 2 + per / 2];
        s.tick_scope(100 /*volume_pct*/, &quiet, SCOPE_TEST_SR);
        s.tick_scope(100 /*volume_pct*/, &loud, SCOPE_TEST_SR);
        assert_eq!(s.wave.len(), 4, "半列尾巴不该出列");
        assert_eq!(s.wave_carry.len(), per / 2, "尾巴应留在 carry");
        let newest = s
            .wave_span_from_newest(0)
            .ok_or_else(|| color_eyre::eyre::eyre!("应有最新列"))?;
        let oldest = s
            .wave_span_from_newest(3)
            .ok_or_else(|| color_eyre::eyre::eyre!("应有最旧列"))?;
        assert!((newest.max - 1.0).abs() < 0.01, "最新列应是响批");
        assert!((oldest.max - 0.2).abs() < 0.01, "最旧列应是安静批");
        assert!(s.wave_span_from_newest(4).is_none(), "越界应 None");
        Ok(())
    }

    /// scope 历史环封顶,不无界增长。
    #[test]
    fn scope_history_capped() -> color_eyre::Result<()> {
        let mut s = spectrum_state_with(serde_json::json!({ "tui": { "spectrum": {
            "style": "scope",
            "scope": { "column_ms": 1 },
        } } }))?;
        let per = per_column(&s)?;
        let samples = vec![0.5_f32; per * (super::SCOPE_HIST_CAP + 100)];
        s.tick_scope(100 /*volume_pct*/, &samples, SCOPE_TEST_SR);
        assert_eq!(s.wave.len(), super::SCOPE_HIST_CAP, "环应封顶");
        Ok(())
    }

    /// scope 稳态快照:Braille min/max 包络跨中线,右新左旧;楔形幅度衰减可见。
    #[test]
    fn scope_snapshot() -> color_eyre::Result<()> {
        let mut terminal = Terminal::new(TestBackend::new(80, 10))?;
        let mut state = scope_state()?;
        terminal.draw(|f| super::super::draw(f, f.area(), &state, &Theme::default()))?;
        // 填满面板点宽(字符列 × 2)的包络列,楔形从左(旧,响)衰减到右(新,静)。
        let samples = sine_samples(state.target_bars.get() * 2, per_column(&state)?);
        state.tick_scope(100 /*volume_pct*/, &samples, SCOPE_TEST_SR);
        terminal.draw(|f| super::super::draw(f, f.area(), &state, &Theme::default()))?;
        crate::test_support::assert_snap!(
            "scope 示波器稳态(楔形正弦包络,Braille 跨中线右新左旧)",
            terminal.backend()
        );
        Ok(())
    }

    /// scope 静默:无包络数据也画中线,面板不死寂。
    #[test]
    fn scope_silent_draws_centerline() -> color_eyre::Result<()> {
        let mut terminal = Terminal::new(TestBackend::new(80, 10))?;
        let state = spectrum_state_with(serde_json::json!({ "tui": { "spectrum": {
            "style": "scope",
        } } }))?;
        terminal.draw(|f| super::super::draw(f, f.area(), &state, &Theme::default()))?;
        let buf = terminal.backend().buffer();
        // 面板内区垂直中部应有非空字形(Braille 中线)。
        let mid_y = buf.area().height / 2;
        let has_glyph = (1..buf.area().width - 1).any(|x| {
            buf.cell((x, mid_y))
                .is_some_and(|cell| cell.symbol() != " ")
        });
        assert!(has_glyph, "静默 scope 应画中线");
        Ok(())
    }

    /// 热更 style 改变 tick 行为:bars 态不攒 terrain 历史,reconfigure 成
    /// terrain 后同一实例开始推层(锁 style 现读、非构造期固化)。
    #[test]
    fn reconfigure_style_switches_tick_behavior() -> color_eyre::Result<()> {
        let mut s = spectrum_state()?;
        let n = s.target_bars.get();
        let bars = vec![40_u16; n];
        for _ in 0..8 {
            s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        assert!(s.terrain_hist.is_empty(), "bars 态不该推层");
        let tree = mineral_config::merge_tree(
            mineral_config::default_tree()?,
            serde_json::json!({ "tui": { "spectrum": { "style": "terrain" } } }),
        );
        let cfg = mineral_config::from_tree(&tree)
            .map_err(|w| color_eyre::eyre::eyre!("overlay 落型失败: {w}"))?;
        s.reconfigure(cfg.tui().spectrum().clone(), TICK_MS);
        for _ in 0..8 {
            s.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        assert!(!s.terrain_hist.is_empty(), "热更为 terrain 后应开始推层");
        Ok(())
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
        terminal.draw(|f| super::super::draw(f, f.area(), &state, &Theme::default()))?;
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
        // 先 draw 一帧让渲染层把 target_bars 设成真实内宽。
        terminal.draw(|f| super::super::draw(f, f.area(), &state, &Theme::default()))?;
        let bars = jagged_bars(state.target_bars.get());
        for _ in 0..30 {
            state.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        terminal.draw(|f| super::super::draw(f, f.area(), &state, &Theme::default()))?;
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
        terminal.draw(|f| super::super::draw(f, f.area(), &state, &Theme::default()))?;
        for _ in 0..30 {
            state.tick(false /*playing*/, 100 /*volume_pct*/, None);
        }
        terminal.draw(|f| super::super::draw(f, f.area(), &state, &Theme::default()))?;
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

    /// 集成:条阵渲染真的把 per-column 端点接到了 buffer。
    ///
    /// 字形快照(`assert_snap!` 走 Display)只抓字符、不抓颜色,故颜色覆盖落在这里:
    /// 直接读渲染后 buffer 底行最左 / 最右 cell 的前景色。`Hue` 态全列同色 → 两端相等;
    /// `CoverFixed` 态沿频率轴铺色 → 两端不等。
    #[test]
    fn bars_render_wires_per_column_color() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let mut terminal = Terminal::new(TestBackend::new(80, 10))?;
        let mut state = spectrum_state()?;
        // 先渲一帧让 target_bars = 真实内宽,再喂音频让条形铺满底行。
        terminal.draw(|f| super::super::draw(f, f.area(), &state, &theme))?;
        let bars = jagged_bars(state.target_bars.get());
        for _ in 0..30 {
            state.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }

        terminal.draw(|f| super::super::draw(f, f.area(), &state, &theme))?;
        let hue = bottom_row_edges(terminal.backend())?;
        assert_eq!(hue.left, hue.right, "Hue 态全列同色,底行两端前景应相等");

        // 注入固定色板并推到静止,沿频率轴铺色。
        state.begin_cover_transition(fixed_palette()?, &theme);
        for _ in 0..state.timing.fade_ticks {
            state.tick(true /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        terminal.draw(|f| super::super::draw(f, f.area(), &state, &theme))?;
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
