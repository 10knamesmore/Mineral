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
    ///   不跳回打断目标。任一采样点非真彩(`Color::Rgb`)时无法入带,退化为打断目标色场,
    ///   仅失去中间色连续性。
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

    /// 清空 scope 的连续波形历史(流换代 / 缺口后重置,不混用旧流样本)。
    pub(crate) fn reset_stream(&mut self) {
        self.wave.clear();
        self.wave_carry.clear();
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

    /// 推进配色状态机一拍:
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
    use super::SpectrumState;

    /// 测试注入的主循环帧间隔。
    const TICK_MS: u64 = 16;

    /// 显式选择 bars，避免资源测试随默认样式变化。
    fn spectrum_state() -> color_eyre::Result<SpectrumState> {
        spectrum_state_with(serde_json::json!({ "tui": { "spectrum": {
            "style": "bars",
        } } }))
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

    /// waterfall 按注入的 64ms 间隔聚合历史行，内容跟随输入音量。
    #[test]
    fn waterfall_pushes_rows_on_cadence() -> color_eyre::Result<()> {
        let mut s = spectrum_state_with(serde_json::json!({ "tui": { "spectrum": {
            "style": "waterfall",
            "waterfall": { "push_ms": 64 },
        } } }))?;
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

    /// terrain 暂停后保留历史层，不追加来自旧 FFT 窗口的数据。
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
        for _ in 0..32 {
            s.tick(false /*playing*/, 100 /*volume_pct*/, None);
            s.tick(false /*playing*/, 100 /*volume_pct*/, Some(&bars));
        }
        assert_eq!(s.terrain_hist, hist_before, "暂停期不该推层");
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
        assert_eq!(s.terrain_hist.len(), *s.cfg.terrain().layers());
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

    /// 造恰好聚成 `column_count` 列的正弦样本，幅度从旧样本向新样本递减。
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
}
