//! 全屏沉浸页的氛围背景:封面调色板驱动的渐变场。
//!
//! [`AmbientGradient`] 是调色板过渡状态机(与动态 accent 同一 from/to/frame 范式):
//! 持有过渡起点的锚点色与目标色板,可见色逐锚点在 Lab 空间插值;打断冻结当前可见色
//! 不跳变、retempo 保相位。[`render`] 是纯函数:锚点高斯混合出每 cell 的场色,按浓度
//! 从底色向场色走、边缘叠暗角,**只写 `bg` 不动字符与 `fg`**——后画的面板文字
//! (fg-only style 是补丁语义)天然叠加其上,无需任何组件配合。
//!
//! 锚点表 / σ / 暗角等观感数值全部现读配置(`tui.ambient`),状态机只携带
//! 「色过渡进度 + 漂移时钟 + 轮转相位」三样运行态,锚点热更下一帧即生效。
//!
//! [`LoudnessPulse`] 是独立的响度包络:播放中的 PCM 样本每拍喂入,输出平滑响度
//! 供 [`render`] 叠加进场浓度——音乐越响封面色越浓,随鼓点呼吸。

use mineral_config::{AmbientConfig, AnchorConfig, PulseConfig};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::render::palette::{CoverPalette, Rgb};
use crate::runtime::cover::colors::lerp_lab;

/// 氛围渐变状态机:从「切换那刻的可见锚点色」在 Lab 空间过渡到目标色板。
///
/// 与动态 accent 并行驱动(同一个封面身份 diff 触发),时长独立(配置
/// `ambient.fade_ms`)。打断(渐变途中换目标)把当前插值色冻结为新起点,不跳变。
/// 漂移时钟 / 轮转相位与色过渡无关,各自开着就随 tick 前进。
#[derive(Clone, Debug)]
pub struct AmbientGradient {
    /// 过渡起点锚点色(设目标那刻的可见色,已冻结);`None` = 底色场(启动初态)。
    from: Option<Vec<Rgb>>,

    /// 过渡目标:`Some` = 封面色板(锚点色 = 色板在其**轮转后采样位**的取色,随
    /// [`Self::rotate_phase`] 流动),`None` = 回落底色场。
    to: Option<CoverPalette>,

    /// 已过渡拍数,推进到 `fade_ticks` 后静止。
    frame: u32,

    /// 全程拍数(`ambient.fade_ms` 按帧率折算,恒 ≥ 1)。
    fade_ticks: u32,

    /// 漂移时钟(秒,已含 `drift.speed` 倍率):锚点角位置 = 时钟 × 各自角速度 + 初相。
    /// 单调累加;f32 在天级时长下精度衰减仅致亚 cell 级摆幅抖动,可忽略。
    drift_t: f32,

    /// 颜色轮转相位(三角波域 `0..2`,一整圈 = `rotate.cycle_secs`;绕 2 回绕故
    /// 长跑不失精度)。
    rotate_phase: f32,

    /// 每拍秒数(`animation.frame_tick_ms` 折算;漂移 / 轮转推进的步长基准)。
    tick_secs: f32,
}

impl AmbientGradient {
    /// 构造一个已静止在底色场上的状态机(启动初态:不铺场)。
    ///
    /// # Params:
    ///   - `fade_ticks`: 渐变全程拍数(`ambient.fade_ms` 折算;`0` 提为 `1`)
    ///   - `tick_ms`: 主循环帧间隔毫秒(漂移 / 轮转时钟的步长基准)
    pub fn new(fade_ticks: u32, tick_ms: u64) -> Self {
        let fade_ticks = fade_ticks.max(1);
        Self {
            from: None,
            to: None,
            frame: fade_ticks,
            fade_ticks,
            drift_t: 0.0,
            rotate_phase: 0.0,
            tick_secs: secs_of(tick_ms),
        }
    }

    /// 设置新的渐变目标:把**当前可见锚点色**冻结为起点,进度归零。
    /// 同目标重复投喂是空操作(身份 diff 在 app 层,这里再兜一层防热更路径重启)。
    ///
    /// # Params:
    ///   - `palette`: `Some` = 封面色板(锚点按各自采样位取色);`None` = 渐变回底色场
    ///   - `base`: 现行主题底色(冻结当前可见色用)
    ///   - `anchors`: 现行锚点表(冻结起点按它采样)
    pub fn set_target(
        &mut self,
        palette: Option<&CoverPalette>,
        base: Rgb,
        anchors: &[AnchorConfig],
    ) {
        let to = palette.cloned();
        if to == self.to {
            return;
        }
        // 冻结起点不含响度亮端推:推是渲染期瞬态,烙进起点会让切歌那帧的基准色偏亮。
        self.from = Some(self.anchor_colors(base, anchors, /*pos_push*/ 0));
        self.to = to;
        self.frame = 0;
    }

    /// 推进一拍:色过渡进度饱和推进;漂移时钟与轮转相位各按其参数前进。
    ///
    /// # Params:
    ///   - `drift_speed`: 漂移速率倍率(配置 `ambient.drift.speed`;`0` / 关闭 = 时钟冻结)
    ///   - `rotate_cycle_secs`: 轮转整圈秒数(配置 `ambient.rotate.cycle_secs`;
    ///     `0` / 关闭 = 相位冻结)
    pub fn tick(&mut self, drift_speed: f32, rotate_cycle_secs: f32) {
        self.frame = self.frame.saturating_add(1).min(self.fade_ticks);
        if drift_speed.is_finite() && drift_speed > 0.0 {
            self.drift_t += self.tick_secs * drift_speed;
        }
        if rotate_cycle_secs.is_finite() && rotate_cycle_secs > 0.0 {
            self.rotate_phase =
                (self.rotate_phase + 2.0 * self.tick_secs / rotate_cycle_secs).rem_euclid(2.0);
        }
    }

    /// 重设全程拍数 / 帧间隔而**保留相位**(进度比例不变):配置热更 `fade_ms` /
    /// `frame_tick_ms` 时调用,渐变不回跳、只换后续速度。
    ///
    /// # Params:
    ///   - `fade_ticks`: 新全程拍数(`0` 提为 `1`)
    ///   - `tick_ms`: 新帧间隔毫秒
    pub fn retempo(&mut self, fade_ticks: u32, tick_ms: u64) {
        let fade_ticks = fade_ticks.max(1);
        let scaled = u64::from(self.frame).saturating_mul(u64::from(fade_ticks))
            / u64::from(self.fade_ticks.max(1));
        self.frame = u32::try_from(scaled).unwrap_or(fade_ticks).min(fade_ticks);
        self.fade_ticks = fade_ticks;
        self.tick_secs = secs_of(tick_ms);
    }

    /// 已静止在底色场(无封面目标且渐变到程)。渲染方据此在功能关闭时整段跳过铺场。
    pub fn settled_at_base(&self) -> bool {
        self.settled() && self.to.is_none()
    }

    /// 当前可见锚点色(与 `anchors` 同序同长):静止在终点,或起点 → 终点按进度逐
    /// 锚点 Lab 插值。终点 = 色板在「锚点采样位经轮转相位映射、再加响度亮端推
    /// `pos_push`(‰,顶到最亮端为止)」处的取色,`None` 时现读底色(渐变途中热更
    /// 主题即追新底色);起点缺位(锚点表热更变长)补底色。亮端推是逐帧瞬态,
    /// 不烙进冻结起点(见 [`Self::set_target`])。
    fn anchor_colors(&self, base: Rgb, anchors: &[AnchorConfig], pos_push: u32) -> Vec<Rgb> {
        let end_at = |anchor: &AnchorConfig| -> Rgb {
            self.to.as_ref().map_or(base, |palette| {
                palette.sample_rgb(
                    rotated_pos(*anchor.pos(), self.rotate_phase)
                        .saturating_add(pos_push)
                        .min(1000),
                )
            })
        };
        if self.settled() {
            return anchors.iter().map(end_at).collect();
        }
        let start_at = |i: usize| -> Rgb {
            self.from
                .as_ref()
                .map_or(base, |band| band.get(i).copied().unwrap_or(base))
        };
        let permille = u16::try_from(
            u64::from(self.frame).saturating_mul(1000) / u64::from(self.fade_ticks.max(1)),
        )
        .unwrap_or(1000);
        anchors
            .iter()
            .enumerate()
            .map(|(i, anchor)| lerp_lab(start_at(i), end_at(anchor), permille))
            .collect()
    }

    /// 渐变是否已到程(静止)。
    fn settled(&self) -> bool {
        self.frame >= self.fade_ticks
    }
}

/// 响度包络:PCM 样本(tap 已 mono 化)每拍喂入,输出 0..=1000‰ 的平滑响度,
/// 驱动氛围场浓度随音乐呼吸。链路:低通加权(只听底鼓 / 贝斯,人声与镲片不触发
/// 跳动)→ RMS → 峰 / 谷双端归一(近期峰值与谷值都跟踪,当前响度在两者之间定位
/// ——把压限压扁的动态重新撑开,不同母带响度的歌跳动幅度一致)→ 感知 gamma →
/// 主包络(attack/release 快起慢落)与瞬态通道(零 attack 快 release,专抓鼓点
/// 的「点」)取较大者。参数全部现读配置,热更下一拍生效。
#[derive(Clone, Debug)]
pub struct LoudnessPulse {
    /// 主包络(0..=1):持续响度的「呼吸」。
    level: f32,

    /// 瞬态包络(0..=1):零 attack 快 release,乘 `punch.gain` 后与主包络取较大者。
    punch: f32,

    /// 峰值跟踪:瞬间顶起、按 `gain_window_secs` 向当前响度回落。
    slow_peak: f32,

    /// 谷值跟踪:瞬间坠落、按 `gain_window_secs` 向当前响度爬升。
    slow_floor: f32,

    /// 低通滤波器状态(跨拍延续,样本流在拍边界上连续)。
    lowpass_state: f32,

    /// 每拍秒数(`animation.frame_tick_ms` 折算;平滑系数的时间基准)。
    tick_secs: f32,
}

impl LoudnessPulse {
    /// 响度跟踪下限:峰值或峰谷差低于此视作静音 / 无动态,不做归一——否则长
    /// 静音后底噪、或恒定音量的间隙噪声会被归一拉成满幅跳动。
    const PEAK_FLOOR: f32 = 1e-3;

    /// 构造静默初态(包络 0、峰谷跟踪空)。
    ///
    /// # Params:
    ///   - `tick_ms`: 主循环帧间隔毫秒(平滑系数的时间基准)
    pub fn new(tick_ms: u64) -> Self {
        Self {
            level: 0.0,
            punch: 0.0,
            slow_peak: 0.0,
            slow_floor: 0.0,
            lowpass_state: 0.0,
            tick_secs: secs_of(tick_ms),
        }
    }

    /// 重设帧间隔而保留包络与峰谷跟踪(配置热更 `frame_tick_ms` 时调用,不跳变)。
    pub fn retempo(&mut self, tick_ms: u64) {
        self.tick_secs = secs_of(tick_ms);
    }

    /// 喂入本拍的 PCM 样本推进包络一步。空样本(暂停 / 断流)按静音处理,包络
    /// 经 release 自然回落。
    ///
    /// # Params:
    ///   - `samples`: 本拍新到的样本(f32 PCM,tap 侧已 mono 化)
    ///   - `sample_rate`: 采样率 Hz(低通截止的折算基准;`0` 视作未知,跳过低通)
    ///   - `cfg`: 响度跳动配置(全部旋钮现读)
    pub fn feed(&mut self, samples: &[f32], sample_rate: u32, cfg: &PulseConfig) {
        let rms = self.weighted_rms(samples, sample_rate, *cfg.bass_cutoff_hz());
        let window_alpha = alpha_of(self.tick_secs, cfg.gain_window_secs().max(0.1));
        // 峰值瞬升缓落、谷值瞬落缓升,都向当前响度收敛:一段安静后峰谷自动收窄,
        // 呼吸幅度跟着段落走。
        self.slow_peak = if rms > self.slow_peak {
            rms
        } else {
            self.slow_peak + (rms - self.slow_peak) * window_alpha
        };
        self.slow_floor = if rms < self.slow_floor {
            rms
        } else {
            self.slow_floor + (rms - self.slow_floor) * window_alpha
        };
        let span = self.slow_peak - self.slow_floor;
        let normalized = if self.slow_peak > Self::PEAK_FLOOR && span > Self::PEAK_FLOOR {
            ((rms - self.slow_floor) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let target = normalized.powf(cfg.gamma().clamp(0.25, 4.0));
        self.level = envelope_step(
            self.level,
            target,
            self.tick_secs,
            *cfg.attack_ms(),
            *cfg.release_ms(),
        );
        self.punch = envelope_step(
            self.punch,
            target,
            self.tick_secs,
            /*attack_ms*/ 0,
            *cfg.punch().release_ms(),
        );
    }

    /// 当前驱动值的千分比(0..=1000),交给 [`render`] 的 `pulse_permille`:
    /// 主包络与「瞬态包络 × `punch.gain`」取较大者。
    pub fn level_permille(&self, cfg: &PulseConfig) -> u16 {
        let mixed = self
            .level
            .max(self.punch * cfg.punch().gain().clamp(0.0, 1.0));
        u16::try_from(permille_of(mixed * 1000.0)).unwrap_or(1000)
    }

    /// 低通加权 RMS:`cutoff_hz > 0` 且采样率已知时样本先过 one-pole 低通
    /// (滤波器状态跨拍延续),响度只计低频能量;否则全频段 RMS。
    fn weighted_rms(&mut self, samples: &[f32], sample_rate: u32, cutoff_hz: f32) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        if cutoff_hz <= 0.0 || sample_rate == 0 {
            return rms_of(samples);
        }
        let rate = rate_f32(sample_rate);
        let alpha = 1.0 - (-std::f32::consts::TAU * cutoff_hz.min(rate / 2.0) / rate).exp();
        let mut state = self.lowpass_state;
        let mut sum = 0.0_f32;
        for &sample in samples {
            state += alpha * (sample - state);
            sum += state * state;
        }
        self.lowpass_state = state;
        mean_sqrt(sum, samples.len())
    }
}

/// attack/release 非对称一阶平滑推进一步:目标高于现值走 attack,否则走 release;
/// 时长 0 = 一拍到位。
///
/// # Params:
///   - `current`: 现值
///   - `target`: 本拍目标
///   - `tick_secs`: 拍长秒数
///   - `attack_ms`: 上行时间常数,毫秒
///   - `release_ms`: 下行时间常数,毫秒
///
/// # Return:
///   推进后的值。
fn envelope_step(
    current: f32,
    target: f32,
    tick_secs: f32,
    attack_ms: u32,
    release_ms: u32,
) -> f32 {
    let tau_ms = if target > current {
        attack_ms
    } else {
        release_ms
    };
    let tau_secs = secs_of(u64::from(tau_ms));
    let alpha = if tau_secs > 0.0 {
        1.0 - (-tick_secs / tau_secs).exp()
    } else {
        1.0
    };
    current + alpha * (target - current)
}

/// 一拍时长对时间常数 `tau_secs` 的一阶平滑系数(`1 - e^(-dt/τ)`)。
fn alpha_of(tick_secs: f32, tau_secs: f32) -> f32 {
    1.0 - (-tick_secs / tau_secs).exp()
}

/// 样本均方根(空样本 = 静音)。
fn rms_of(samples: &[f32]) -> f32 {
    let sum = samples.iter().map(|s| s * s).sum::<f32>();
    mean_sqrt(sum, samples.len())
}

/// `sqrt(sum / n)`(`n = 0` 给 0)。
#[allow(clippy::as_conversions)] // reason: 样本计数 → f32 只作分母,精度损失可忽略
fn mean_sqrt(sum: f32, n: usize) -> f32 {
    if n == 0 {
        return 0.0;
    }
    (sum / n as f32).sqrt()
}

/// 采样率 → f32(音频采样率 ≤ 192k,f32 内精确)。
#[allow(clippy::as_conversions)] // reason: 采样率量级 < 2^24,f32 表示无损
fn rate_f32(sample_rate: u32) -> f32 {
    sample_rate as f32
}

/// 把氛围渐变场写进 `area` 内每个 cell 的 `bg`(不动字符与 `fg`)。
///
/// 每 cell:各锚点按高斯权重混色(权重归一 `Σw·c / Σw`)→ 按浓度从底色向场色走
/// → 距屏心越远越向底色收敛(暗角,保边缘区文字可读)。锚点位置 = 锚位 + 摆幅 ×
/// 漂移时钟正弦;锚点颜色随轮转相位沿色带流动。锚点色一帧算一次,逐 cell 只做权重混合。
///
/// # Params:
///   - `area`: 铺场区域(整屏;宽高任一为 0 直接返回)
///   - `gradient`: 调色板过渡状态机(锚点色 + 漂移时钟 + 轮转相位)
///   - `base`: 主题底色(须真彩;ANSI 主题由调用方经 [`rgb_of`] 拦下)
///   - `cfg`: 氛围段配置(σ / 暗角 / 摆幅 / 锚点表,现读)
///   - `progress_permille`: 全屏形变进度(‰):浓度乘它,进场随形变淡入、退场收干净
///   - `pulse_permille`: 响度包络(‰,[`LoudnessPulse::level_permille`]):
///     `pulse.enabled` 时按 `pulse.depth` 叠加进浓度,音乐越响场越浓;关闭时忽略
///   - `skip`: 不铺的洞(将被不透明终端图协议真图盖住的封面区)。图协议把整段载荷
///     藏在图区首 cell 的 symbol 里,逐帧改那格 bg 会让 diff 每帧重发载荷——
///     iTerm2 / sixel(数据即显示、自带擦行)表现为整图闪烁;图不透明,跳过零视觉损失
#[allow(clippy::too_many_arguments)] // reason: 纯渲染入口,参数即全部输入,收拢成 struct 反而多一层搬运
pub fn render(
    buf: &mut Buffer,
    area: Rect,
    gradient: &AmbientGradient,
    base: Rgb,
    cfg: &AmbientConfig,
    progress_permille: u16,
    pulse_permille: u16,
    skip: Option<Rect>,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    /// 一个已定位定色的锚点(帧内预折算,逐 cell 循环只读)。
    struct Blob {
        /// 本帧横坐标(锚位 + 摆幅偏移)。
        x: f32,
        /// 本帧纵坐标。
        y: f32,
        /// 红分量(0..=255 浮点)。
        r: f32,
        /// 绿分量。
        g: f32,
        /// 蓝分量。
        b: f32,
    }
    let anchors = cfg.anchors();
    let pulse_cfg = cfg.pulse();
    let pulse = if *pulse_cfg.enabled() {
        f32::from(pulse_permille.min(1000)) / 1000.0
    } else {
        0.0
    };
    let depth = pulse_cfg.depth();
    let boost = |d: f32| d.clamp(0.0, 1.0) * pulse;
    let colors = gradient.anchor_colors(
        base,
        anchors,
        /*pos_push*/ permille_of(boost(*depth.brightness()) * 1000.0),
    );
    let sway = *cfg.drift().sway_pct() / 100.0;
    let t = gradient.drift_t;
    let blobs = anchors
        .iter()
        .zip(colors)
        .map(|(anchor, c)| Blob {
            x: anchor.x() + sway * (t * anchor.speed_x() + anchor.phase_x()).sin(),
            y: anchor.y() + sway * (t * anchor.speed_y() + anchor.phase_y()).cos(),
            r: f32::from(c.r),
            g: f32::from(c.g),
            b: f32::from(c.b),
        })
        .collect::<Vec<Blob>>();
    let (base_r, base_g, base_b) = (f32::from(base.r), f32::from(base.g), f32::from(base.b));
    let (grid_w, grid_h) = (f32::from(area.width), f32::from(area.height));
    let intensity = ((cfg.intensity() + boost(*depth.intensity()))
        * f32::from(progress_permille.min(1000))
        / 1000.0)
        .clamp(0.0, 1.0);
    let vignette = cfg.vignette();
    let veil_strength = (vignette.strength() * (1.0 - boost(*depth.vignette()))).clamp(0.0, 1.0);
    let veil_inner = *vignette.inner();
    // 满强半径贴着起始半径也不除零:压出一段极窄的过渡带。
    let veil_span = (vignette.outer() - veil_inner).max(1e-3);
    let sigma = (cfg.sigma() * (1.0 + boost(*depth.sigma()))).max(1e-3);
    let inv_two_sigma_sq = 1.0 / (2.0 * sigma * sigma);
    for cy in 0..area.height {
        let ny = (f32::from(cy) + 0.5) / grid_h;
        for cx in 0..area.width {
            if skip.is_some_and(|hole| {
                hole.contains(ratatui::layout::Position::new(area.x + cx, area.y + cy))
            }) {
                continue;
            }
            let nx = (f32::from(cx) + 0.5) / grid_w;
            let (mut wsum, mut r, mut g, mut b) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
            for blob in &blobs {
                let (dx, dy) = (nx - blob.x, ny - blob.y);
                let w = (-(dx * dx + dy * dy) * inv_two_sigma_sq).exp();
                wsum += w;
                r += blob.r * w;
                g += blob.g * w;
                b += blob.b * w;
            }
            // exp 无零点故有锚点时 wsum 恒正;空锚点表 / 极端下溢退底色。
            let field = if wsum > f32::MIN_POSITIVE {
                (r / wsum, g / wsum, b / wsum)
            } else {
                (base_r, base_g, base_b)
            };
            let (dx, dy) = (nx - 0.5, ny - 0.5);
            let dist = (dx * dx + dy * dy).sqrt();
            let veil = ((dist - veil_inner) / veil_span).clamp(0.0, 1.0) * veil_strength;
            let mix = intensity * (1.0 - veil);
            let color = Color::Rgb(
                quantize(base_r + (field.0 - base_r) * mix),
                quantize(base_g + (field.1 - base_g) * mix),
                quantize(base_b + (field.2 - base_b) * mix),
            );
            if let Some(cell) = buf.cell_mut((area.x + cx, area.y + cy)) {
                cell.set_bg(color);
            }
        }
    }
}

/// 从渲染色提取 sRGB 分量:氛围场要做颜色数学,仅真彩 `Color::Rgb` 可用
/// (ANSI / indexed 主题拿不到分量,调用方据 `None` 跳过铺场,优雅降级)。
pub fn rgb_of(color: Color) -> Option<Rgb> {
    match color {
        Color::Rgb(r, g, b) => Some(Rgb::new(r, g, b)),
        _ => None,
    }
}

/// 锚点采样位经颜色轮转相位映射:沿色带「0 → 1000 → 0」三角波往返。色带按明度升序、
/// 非环形,直接回绕会出现「最亮 → 最暗」跳变;往返无缝。`pos` 充当初相,各锚点保持
/// 相对错开(相位 0 时恒等,即轮转关闭 = 钉在配置位)。
fn rotated_pos(pos: u32, phase: f32) -> u32 {
    let offset = f32::from(u16::try_from(pos.min(1000)).unwrap_or(1000)) / 1000.0;
    let x = (offset + phase).rem_euclid(2.0);
    permille_of((1.0 - (1.0 - x).abs()) * 1000.0)
}

/// `0..=1000` 浮点量化回 u32 千分比。
#[allow(clippy::as_conversions)] // reason: 已 clamp 进 0..=1000 且 round,转换语义无损
fn permille_of(v: f32) -> u32 {
    v.clamp(0.0, 1000.0).round() as u32
}

/// `0..=255` 浮点分量量化回 u8。
#[allow(clippy::as_conversions)] // reason: 已 clamp 进 0..=255 且 round,转换语义无损
fn quantize(v: f32) -> u8 {
    v.clamp(0.0, 255.0).round() as u8
}

/// 毫秒 → 秒(f32)。分量先收进 u16(帧间隔现实上限内)再无损转 f32,不触 `as`。
fn secs_of(tick_ms: u64) -> f32 {
    f32::from(u16::try_from(tick_ms).unwrap_or(u16::MAX)) / 1000.0
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use color_eyre::eyre::eyre;
    use mineral_config::AmbientConfig;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};

    use super::{AmbientGradient, LoudnessPulse, render, rgb_of, rotated_pos};
    use crate::render::palette::{CoverPalette, Rgb};
    use crate::runtime::cover::colors::lerp_lab;

    /// 测试底色(mocha base 同值,便于人查)。
    const BASE: Rgb = Rgb {
        r: 0x1e,
        g: 0x1e,
        b: 0x2e,
    };

    /// 「default.lua + ambient 段 overlay」合成 [`AmbientConfig`](测试对配置旋钮的
    /// 唯一入口,与热更推送同构)。
    fn acfg(overlay: &serde_json::Value) -> color_eyre::Result<AmbientConfig> {
        let tree = mineral_config::merge_tree(
            mineral_config::default_tree()?,
            serde_json::json!({ "tui": { "ambient": overlay } }),
        );
        Ok(mineral_config::from_tree(&tree)
            .map_err(|warning| eyre!("测试配置落型失败:{warning}"))?
            .tui()
            .ambient()
            .clone())
    }

    /// 几何项归零的观感配置:全浓度、无暗角、无摆幅,只看色场。
    fn flat_cfg() -> color_eyre::Result<AmbientConfig> {
        acfg(&serde_json::json!({
            "intensity": 1.0,
            "vignette": { "strength": 0.0 },
            "drift": { "sway_pct": 0.0 },
        }))
    }

    /// 造非空色板。
    fn palette(swatches: Vec<Rgb>) -> color_eyre::Result<CoverPalette> {
        CoverPalette::new(swatches).ok_or_else(|| eyre!("非空应构造成功"))
    }

    /// 双色板(暗蓝 → 亮红):锚点色带采样有梯度,便于断言过渡。
    fn blue_red() -> color_eyre::Result<CoverPalette> {
        palette(vec![Rgb::new(20, 20, 120), Rgb::new(220, 60, 60)])
    }

    /// 期望的到程锚点色(轮转相位 0:各锚点钉在配置 `pos` 采样)。
    fn expected_band(pal: &CoverPalette, cfg: &AmbientConfig) -> Vec<Rgb> {
        cfg.anchors()
            .iter()
            .map(|anchor| pal.sample_rgb(*anchor.pos()))
            .collect()
    }

    /// 初态即静止在底色场:`settled_at_base` 真,锚点色全为底色。
    #[test]
    fn new_settles_at_base() -> color_eyre::Result<()> {
        let cfg = flat_cfg()?;
        let g = AmbientGradient::new(/*fade_ticks*/ 10, /*tick_ms*/ 16);
        assert!(g.settled_at_base());
        assert_eq!(
            g.anchor_colors(BASE, cfg.anchors(), /*pos_push*/ 0),
            vec![BASE; cfg.anchors().len()]
        );
        Ok(())
    }

    /// 设目标:起点 = 底色,中点 = 逐锚点 Lab 插值,到程 = 色板锚点采样;到程后静止。
    #[test]
    fn fades_to_band_then_settles() -> color_eyre::Result<()> {
        let cfg = flat_cfg()?;
        let pal = blue_red()?;
        let mut g = AmbientGradient::new(/*fade_ticks*/ 10, /*tick_ms*/ 16);
        g.set_target(Some(&pal), BASE, cfg.anchors());
        assert_eq!(
            g.anchor_colors(BASE, cfg.anchors(), /*pos_push*/ 0),
            vec![BASE; cfg.anchors().len()],
            "frame 0 应从底色起步"
        );
        for _ in 0..5 {
            g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        }
        let mid = g.anchor_colors(BASE, cfg.anchors(), /*pos_push*/ 0);
        let band = expected_band(&pal, &cfg);
        for (got, end) in mid.into_iter().zip(band.iter().copied()) {
            assert_eq!(got, lerp_lab(BASE, end, 500), "中点应是 Lab 半程插值");
        }
        for _ in 0..5 {
            g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        }
        assert_eq!(
            g.anchor_colors(BASE, cfg.anchors(), /*pos_push*/ 0),
            band,
            "到程应达色板锚点采样"
        );
        assert!(!g.settled_at_base(), "有封面目标时不算底色静止");
        Ok(())
    }

    /// 打断:渐变途中换目标,换前后同一帧可见色不跳变(起点冻结)。
    #[test]
    fn retarget_freezes_current_no_jump() -> color_eyre::Result<()> {
        let cfg = flat_cfg()?;
        let mut g = AmbientGradient::new(/*fade_ticks*/ 10, /*tick_ms*/ 16);
        g.set_target(Some(&blue_red()?), BASE, cfg.anchors());
        for _ in 0..4 {
            g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        }
        let before = g.anchor_colors(BASE, cfg.anchors(), /*pos_push*/ 0);
        let green = palette(vec![Rgb::new(20, 120, 20)])?;
        g.set_target(Some(&green), BASE, cfg.anchors());
        assert_eq!(
            g.anchor_colors(BASE, cfg.anchors(), /*pos_push*/ 0),
            before,
            "打断那帧不应跳色"
        );
        Ok(())
    }

    /// 回落:静止在色板上后目标置 `None`,渐变回底色场并 `settled_at_base`。
    #[test]
    fn fades_back_to_base() -> color_eyre::Result<()> {
        let cfg = flat_cfg()?;
        let n = cfg.anchors().len();
        let mut g = AmbientGradient::new(/*fade_ticks*/ 10, /*tick_ms*/ 16);
        g.set_target(Some(&blue_red()?), BASE, cfg.anchors());
        for _ in 0..10 {
            g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        }
        g.set_target(/*palette*/ None, BASE, cfg.anchors());
        assert_ne!(
            g.anchor_colors(BASE, cfg.anchors(), /*pos_push*/ 0),
            vec![BASE; n],
            "回落起点是封面色,不瞬跳"
        );
        for _ in 0..10 {
            g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        }
        assert_eq!(
            g.anchor_colors(BASE, cfg.anchors(), /*pos_push*/ 0),
            vec![BASE; n]
        );
        assert!(g.settled_at_base(), "回落到程应判定底色静止");
        Ok(())
    }

    /// retempo 保相位:半程改时长,当前帧可见色不动。
    #[test]
    fn retempo_preserves_phase() -> color_eyre::Result<()> {
        let cfg = flat_cfg()?;
        let mut g = AmbientGradient::new(/*fade_ticks*/ 10, /*tick_ms*/ 16);
        g.set_target(Some(&blue_red()?), BASE, cfg.anchors());
        for _ in 0..5 {
            g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        }
        let before = g.anchor_colors(BASE, cfg.anchors(), /*pos_push*/ 0);
        g.retempo(/*fade_ticks*/ 20, /*tick_ms*/ 16);
        assert_eq!(
            g.anchor_colors(BASE, cfg.anchors(), /*pos_push*/ 0),
            before,
            "retempo 不应改变当前帧颜色"
        );
        Ok(())
    }

    /// 同目标重复投喂是空操作:进度不归零(防热更路径重启渐变)。
    #[test]
    fn same_target_does_not_restart() -> color_eyre::Result<()> {
        let cfg = flat_cfg()?;
        let pal = blue_red()?;
        let mut g = AmbientGradient::new(/*fade_ticks*/ 10, /*tick_ms*/ 16);
        g.set_target(Some(&pal), BASE, cfg.anchors());
        for _ in 0..7 {
            g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        }
        let before = g.anchor_colors(BASE, cfg.anchors(), /*pos_push*/ 0);
        g.set_target(Some(&pal), BASE, cfg.anchors());
        assert_eq!(
            g.anchor_colors(BASE, cfg.anchors(), /*pos_push*/ 0),
            before,
            "同目标不应重启渐变"
        );
        Ok(())
    }

    /// `rotated_pos` 三角波:相位 0 恒等;半相位把 0 → 500、500 → 1000、1000 → 500;
    /// 整圈(相位 2)回到恒等——往返全程无「亮 → 暗」回绕跳变。
    #[test]
    fn rotated_pos_triangle_wave() {
        for pos in [0_u32, 250, 500, 750, 1000] {
            assert_eq!(rotated_pos(pos, 0.0), pos, "相位 0 应恒等");
            assert_eq!(rotated_pos(pos, 2.0), pos, "整圈应回到恒等");
        }
        assert_eq!(rotated_pos(0, 0.5), 500);
        assert_eq!(rotated_pos(500, 0.5), 1000, "过顶点后往回走");
        assert_eq!(rotated_pos(1000, 0.5), 500);
        assert_eq!(rotated_pos(0, 1.0), 1000, "半圈把暗端推到亮端");
    }

    /// 颜色轮转:到程静止后推进带轮转的 tick,锚点色仍随相位流动;
    /// `cycle_secs = 0` 则相位冻结、逐锚点不变。
    #[test]
    fn rotation_moves_settled_colors() -> color_eyre::Result<()> {
        let cfg = flat_cfg()?;
        let pal = blue_red()?;
        let mut g = AmbientGradient::new(/*fade_ticks*/ 1, /*tick_ms*/ 16);
        g.set_target(Some(&pal), BASE, cfg.anchors());
        g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        let before = g.anchor_colors(BASE, cfg.anchors(), /*pos_push*/ 0);
        let mut frozen = g.clone();
        // 周期 4s、拍长 16ms:125 拍走完半圈,采样位翻转到对侧,颜色必然变化。
        for _ in 0..125 {
            g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 4.0);
            frozen.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        }
        assert_ne!(
            g.anchor_colors(BASE, cfg.anchors(), /*pos_push*/ 0),
            before,
            "轮转推进后锚点色应流动"
        );
        assert_eq!(
            frozen.anchor_colors(BASE, cfg.anchors(), /*pos_push*/ 0),
            before,
            "轮转关闭应逐锚点冻结"
        );
        Ok(())
    }

    /// 浓度 0:输出恒为底色(等效关闭,渐变场完全不显)。
    #[test]
    fn zero_intensity_paints_flat_base() -> color_eyre::Result<()> {
        let cfg = acfg(&serde_json::json!({ "intensity": 0.0 }))?;
        let mut g = AmbientGradient::new(/*fade_ticks*/ 1, /*tick_ms*/ 16);
        g.set_target(Some(&blue_red()?), BASE, cfg.anchors());
        g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        let area = Rect::new(0, 0, 12, 6);
        let mut buf = Buffer::empty(area);
        render(
            &mut buf, area, &g, BASE, &cfg, /*progress_permille*/ 1000,
            /*pulse_permille*/ 0, /*skip*/ None,
        );
        for (x, y) in cells(area) {
            assert_eq!(
                bg_at(&buf, x, y)?,
                Color::Rgb(BASE.r, BASE.g, BASE.b),
                "({x},{y}) 应为纯底色"
            );
        }
        Ok(())
    }

    /// 权重归一:单色板(各锚点同色)+ 全浓度 + 无暗角 → 每 cell 恰为该色
    /// (高斯权重在分子分母同现,归一后不残留几何痕迹)。
    #[test]
    fn uniform_band_renders_uniform_field() -> color_eyre::Result<()> {
        let cfg = flat_cfg()?;
        let c = Rgb::new(90, 40, 140);
        let mut g = AmbientGradient::new(/*fade_ticks*/ 1, /*tick_ms*/ 16);
        g.set_target(Some(&palette(vec![c])?), BASE, cfg.anchors());
        g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        let area = Rect::new(0, 0, 16, 8);
        let mut buf = Buffer::empty(area);
        render(
            &mut buf, area, &g, BASE, &cfg, /*progress_permille*/ 1000,
            /*pulse_permille*/ 0, /*skip*/ None,
        );
        for (x, y) in cells(area) {
            assert_eq!(
                bg_at(&buf, x, y)?,
                Color::Rgb(c.r, c.g, c.b),
                "({x},{y}) 单色场应逐 cell 等于该色"
            );
        }
        Ok(())
    }

    /// 暗角:满强度下角落 cell 比屏心 cell 更接近底色(逐通道距离严格更小)。
    #[test]
    fn vignette_converges_edges_toward_base() -> color_eyre::Result<()> {
        let cfg = acfg(&serde_json::json!({
            "intensity": 1.0,
            "vignette": { "strength": 1.0 },
            "drift": { "sway_pct": 0.0 },
        }))?;
        let c = Rgb::new(200, 200, 40);
        let mut g = AmbientGradient::new(/*fade_ticks*/ 1, /*tick_ms*/ 16);
        g.set_target(Some(&palette(vec![c])?), BASE, cfg.anchors());
        g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(area);
        render(
            &mut buf, area, &g, BASE, &cfg, /*progress_permille*/ 1000,
            /*pulse_permille*/ 0, /*skip*/ None,
        );
        let dist_to_base = |color: Color| -> color_eyre::Result<u32> {
            let Color::Rgb(r, g, b) = color else {
                return Err(eyre!("应为真彩,实得 {color:?}"));
            };
            Ok(u32::from(r.abs_diff(BASE.r))
                + u32::from(g.abs_diff(BASE.g))
                + u32::from(b.abs_diff(BASE.b)))
        };
        let center = dist_to_base(bg_at(&buf, 10, 5)?)?;
        let corner = dist_to_base(bg_at(&buf, 0, 0)?)?;
        assert!(
            corner < center,
            "角落({corner})应比屏心({center})更接近底色"
        );
        Ok(())
    }

    /// 只写 bg:预置了字符 + fg 的 cell 经铺场后字符与 fg 原样,仅 bg 被覆写。
    #[test]
    fn writes_bg_only() -> color_eyre::Result<()> {
        let cfg = flat_cfg()?;
        let mut g = AmbientGradient::new(/*fade_ticks*/ 1, /*tick_ms*/ 16);
        g.set_target(Some(&blue_red()?), BASE, cfg.anchors());
        g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        let area = Rect::new(0, 0, 8, 4);
        let mut buf = Buffer::empty(area);
        buf.set_string(2, 1, "lyric", Style::new().fg(Color::Rgb(255, 0, 255)));
        render(
            &mut buf, area, &g, BASE, &cfg, /*progress_permille*/ 1000,
            /*pulse_permille*/ 0, /*skip*/ None,
        );
        let cell = buf.cell((2, 1)).ok_or_else(|| eyre!("cell 越界"))?;
        assert_eq!(cell.symbol(), "l", "字符不应被铺场动到");
        assert_eq!(cell.fg, Color::Rgb(255, 0, 255), "fg 不应被铺场动到");
        assert_ne!(cell.bg, Color::Reset, "bg 应被场色覆写");
        Ok(())
    }

    /// 漂移:摆幅 + 速率下推进若干拍后场分布变化(至少一个 cell 的 bg 不同);
    /// 速率 0 则时钟冻结、逐 cell 不变。
    #[test]
    fn drift_moves_field_and_zero_speed_freezes() -> color_eyre::Result<()> {
        let cfg = acfg(&serde_json::json!({
            "intensity": 1.0,
            "vignette": { "strength": 0.0 },
        }))?;
        let pal = blue_red()?;
        let area = Rect::new(0, 0, 24, 10);
        let paint = |g: &AmbientGradient| {
            let mut buf = Buffer::empty(area);
            render(
                &mut buf, area, g, BASE, &cfg, /*progress_permille*/ 1000,
                /*pulse_permille*/ 0, /*skip*/ None,
            );
            buf
        };
        let mut g = AmbientGradient::new(/*fade_ticks*/ 1, /*tick_ms*/ 16);
        g.set_target(Some(&pal), BASE, cfg.anchors());
        g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        let before = paint(&g);
        let mut frozen = g.clone();
        // 1 拍 16ms,推 300 拍 ≈ 4.8s;默认角速度下摆幅位移已跨 cell。
        for _ in 0..300 {
            g.tick(/*drift_speed*/ 1.0, /*rotate_cycle_secs*/ 0.0);
            frozen.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        }
        assert_ne!(
            format!("{:?}", paint(&g)),
            format!("{before:?}"),
            "漂移推进后场分布应变化"
        );
        assert_eq!(
            format!("{:?}", paint(&frozen)),
            format!("{before:?}"),
            "速率 0 应逐 cell 冻结"
        );
        Ok(())
    }

    /// 形变进度缩放:进度 0 时浓度归零(恒底色),进度半程时浓度介于 0 与满程之间。
    #[test]
    fn progress_scales_intensity() -> color_eyre::Result<()> {
        let cfg = flat_cfg()?;
        let c = Rgb::new(200, 60, 60);
        let mut g = AmbientGradient::new(/*fade_ticks*/ 1, /*tick_ms*/ 16);
        g.set_target(Some(&palette(vec![c])?), BASE, cfg.anchors());
        g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        let area = Rect::new(0, 0, 4, 2);
        let probe = |permille: u16| -> color_eyre::Result<Color> {
            let mut buf = Buffer::empty(area);
            render(
                &mut buf, area, &g, BASE, &cfg, permille, /*pulse_permille*/ 0,
                /*skip*/ None,
            );
            bg_at(&buf, 1, 1)
        };
        assert_eq!(
            probe(0)?,
            Color::Rgb(BASE.r, BASE.g, BASE.b),
            "进度 0 应恒底色(形变起点不铺色)"
        );
        assert_eq!(probe(1000)?, Color::Rgb(c.r, c.g, c.b), "满进度达场色");
        let Color::Rgb(mid_r, ..) = probe(500)? else {
            return Err(eyre!("半程应为真彩"));
        };
        assert!(
            mid_r > BASE.r && mid_r < c.r,
            "半程浓度应介于底色与场色之间,实得 r={mid_r}"
        );
        Ok(())
    }

    /// skip 洞:洞内 cell 的 bg 原样不动(终端图协议真图区不铺场,防载荷 cell 每帧脏),
    /// 洞外照常铺场。
    #[test]
    fn skip_hole_leaves_bg_untouched() -> color_eyre::Result<()> {
        let cfg = flat_cfg()?;
        let c = Rgb::new(90, 40, 140);
        let mut g = AmbientGradient::new(/*fade_ticks*/ 1, /*tick_ms*/ 16);
        g.set_target(Some(&palette(vec![c])?), BASE, cfg.anchors());
        g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        let area = Rect::new(0, 0, 12, 6);
        let hole = Rect::new(2, 1, 4, 3);
        let mut buf = Buffer::empty(area);
        render(
            &mut buf,
            area,
            &g,
            BASE,
            &cfg,
            /*progress_permille*/ 1000,
            /*pulse_permille*/ 0,
            Some(hole),
        );
        for (x, y) in cells(area) {
            let inside = hole.contains(ratatui::layout::Position::new(x, y));
            let bg = bg_at(&buf, x, y)?;
            if inside {
                assert_eq!(bg, Color::Reset, "洞内 ({x},{y}) 不应被铺场");
            } else {
                assert_eq!(bg, Color::Rgb(c.r, c.g, c.b), "洞外 ({x},{y}) 照常铺场");
            }
        }
        Ok(())
    }

    /// `rgb_of`:真彩取出分量,ANSI / indexed 主题给 `None`(调用方跳过铺场)。
    #[test]
    fn rgb_of_only_accepts_truecolor() {
        assert_eq!(rgb_of(Color::Rgb(1, 2, 3)), Some(Rgb::new(1, 2, 3)));
        assert_eq!(rgb_of(Color::Blue), None);
        assert_eq!(rgb_of(Color::Indexed(42)), None);
    }

    /// 固定色板 + 默认观感配置的渐变场快照(4bit/通道 hex 网格):锚点几何、浓度
    /// 收敛与暗角形状的回归基线。粗量化吸收 libm 跨平台 ulp 差异。
    #[test]
    fn ambient_field_snapshot() -> color_eyre::Result<()> {
        let cfg = acfg(&serde_json::json!({}))?;
        let pal = palette(vec![
            Rgb::new(20, 20, 120),
            Rgb::new(200, 40, 40),
            Rgb::new(230, 200, 60),
        ])?;
        let mut g = AmbientGradient::new(/*fade_ticks*/ 1, /*tick_ms*/ 16);
        g.set_target(Some(&pal), BASE, cfg.anchors());
        g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        let area = Rect::new(0, 0, 24, 8);
        let mut buf = Buffer::empty(area);
        render(
            &mut buf, area, &g, BASE, &cfg, /*progress_permille*/ 1000,
            /*pulse_permille*/ 0, /*skip*/ None,
        );
        let grid = bg_grid(&buf, area)?;
        mineral_test::assert_snap!(
            "氛围渐变场:三色板稳态 24×8,默认观感配置(bg 压 4bit hex)",
            grid
        );
        Ok(())
    }

    /// 测试用采样率(48kHz)。
    const RATE: u32 = 48_000;

    /// pulse 段配置(默认值 + overlay 合成)。
    fn pulse_cfg(overlay: &serde_json::Value) -> color_eyre::Result<mineral_config::PulseConfig> {
        Ok(acfg(&serde_json::json!({ "pulse": overlay }))?
            .pulse()
            .clone())
    }

    /// 关掉 punch 通道的 pulse 配置(只看主包络的测试用)。
    fn pulse_cfg_no_punch(
        overlay: serde_json::Value,
    ) -> color_eyre::Result<mineral_config::PulseConfig> {
        let mut merged = overlay;
        if let Some(table) = merged.as_object_mut() {
            table.insert("punch".to_owned(), serde_json::json!({ "gain": 0.0 }));
        }
        pulse_cfg(&merged)
    }

    /// 包络快起慢落:同拍数下 attack 的升幅远大于 release 的降幅(跟得上鼓点、
    /// 鼓点间不闪的手感基础)。
    #[test]
    fn pulse_attack_faster_than_release() -> color_eyre::Result<()> {
        let cfg = pulse_cfg_no_punch(serde_json::json!({}))?;
        let mut p = LoudnessPulse::new(/*tick_ms*/ 16);
        let loud = vec![0.5_f32; 512];
        for _ in 0..5 {
            p.feed(&loud, RATE, &cfg);
        }
        let peak = p.level_permille(&cfg);
        for _ in 0..5 {
            p.feed(&[], RATE, &cfg);
        }
        let after = p.level_permille(&cfg);
        assert!(after < peak, "静音后应回落");
        let drop = peak - after;
        assert!(
            peak > drop * 2,
            "同拍数 attack 升幅({peak})应远大于 release 降幅({drop})"
        );
        Ok(())
    }

    /// 峰谷归一:恒定小音量按自身峰谷定标,包络仍到近满幅(母带响度差异不影响
    /// 跳动幅度)。
    #[test]
    fn pulse_gain_normalizes_quiet_input() -> color_eyre::Result<()> {
        let cfg = pulse_cfg(&serde_json::json!({}))?;
        let mut p = LoudnessPulse::new(/*tick_ms*/ 16);
        let quiet = vec![0.01_f32; 512];
        for _ in 0..60 {
            p.feed(&quiet, RATE, &cfg);
        }
        assert!(
            p.level_permille(&cfg) > 900,
            "恒定小音量应被归一到近满幅,实得 {}",
            p.level_permille(&cfg)
        );
        Ok(())
    }

    /// 静音与底噪:空样本包络归零;RMS 低于跟踪下限的底噪不被归一拉起。
    #[test]
    fn pulse_silence_and_noise_floor_stay_zero() -> color_eyre::Result<()> {
        let cfg = pulse_cfg(&serde_json::json!({}))?;
        let mut p = LoudnessPulse::new(/*tick_ms*/ 16);
        for _ in 0..30 {
            p.feed(&[], RATE, &cfg);
        }
        assert_eq!(p.level_permille(&cfg), 0);
        let noise = vec![1e-4_f32; 512];
        for _ in 0..30 {
            p.feed(&noise, RATE, &cfg);
        }
        assert_eq!(p.level_permille(&cfg), 0, "底噪不应被归一拉成满幅");
        Ok(())
    }

    /// retempo 保状态:改帧间隔的瞬间包络值不变;此后同样样本下新拍长(更长)
    /// 单拍步进更大。
    #[test]
    fn pulse_retempo_preserves_level_then_rescales_step() -> color_eyre::Result<()> {
        let cfg = pulse_cfg_no_punch(serde_json::json!({}))?;
        let mut p = LoudnessPulse::new(/*tick_ms*/ 16);
        let loud = vec![0.5_f32; 64];
        for _ in 0..3 {
            p.feed(&loud, RATE, &cfg);
        }
        let mut stale = p.clone();
        let before = p.level_permille(&cfg);
        p.retempo(/*tick_ms*/ 32);
        assert_eq!(p.level_permille(&cfg), before, "retempo 瞬间包络不跳");
        p.feed(&loud, RATE, &cfg);
        stale.feed(&loud, RATE, &cfg);
        assert!(
            p.level_permille(&cfg) > stale.level_permille(&cfg),
            "拍长翻倍后单拍升幅应更大:{} vs {}",
            p.level_permille(&cfg),
            stale.level_permille(&cfg)
        );
        Ok(())
    }

    /// 低通加权:同幅度下 50Hz 正弦驱动包络到近满幅,8kHz 正弦被低通滤波衰到
    /// 静音线以下不触发跳动(镲片 / 齿音不该闪)。
    #[test]
    fn pulse_bass_weighting_ignores_highs() -> color_eyre::Result<()> {
        let cfg = pulse_cfg_no_punch(serde_json::json!({ "attack_ms": 0, "release_ms": 0 }))?;
        let sine = |freq: f32| -> Vec<f32> {
            (0..512_u16)
                .map(|i| (std::f32::consts::TAU * freq * f32::from(i) / 48_000.0).sin() * 0.05)
                .collect::<Vec<f32>>()
        };
        let mut low = LoudnessPulse::new(/*tick_ms*/ 16);
        let mut high = LoudnessPulse::new(/*tick_ms*/ 16);
        for _ in 0..10 {
            low.feed(&sine(50.0), RATE, &cfg);
            high.feed(&sine(8_000.0), RATE, &cfg);
        }
        assert!(
            low.level_permille(&cfg) > 800,
            "低频应驱动近满幅,实得 {}",
            low.level_permille(&cfg)
        );
        assert_eq!(high.level_permille(&cfg), 0, "高频应被低通衰到静音线以下");
        Ok(())
    }

    /// 峰谷双端归一:重压限母带(RMS 只在 0.8-1.0 间起伏)被撑开到近满幅摆动
    /// ——谷 ≈ 0、峰 ≈ 1,而非线性比例的 0.8 / 1.0。
    #[test]
    fn pulse_expands_compressed_dynamics() -> color_eyre::Result<()> {
        let cfg = pulse_cfg_no_punch(serde_json::json!({
            "attack_ms": 0,
            "release_ms": 0,
            "gamma": 1.0,
            "bass_cutoff_hz": 0.0,
            "gain_window_secs": 0.5,
        }))?;
        let mut p = LoudnessPulse::new(/*tick_ms*/ 16);
        let loud = vec![1.0_f32; 256];
        let quiet = vec![0.8_f32; 256];
        let (mut top, mut bottom) = (0_u16, 1000_u16);
        for _ in 0..20 {
            for _ in 0..4 {
                p.feed(&loud, RATE, &cfg);
                top = top.max(p.level_permille(&cfg));
            }
            for _ in 0..4 {
                p.feed(&quiet, RATE, &cfg);
                bottom = bottom.min(p.level_permille(&cfg));
            }
        }
        assert!(top > 900, "峰应近满幅,实得 {top}");
        assert!(bottom < 100, "谷应近归零(动态被撑开),实得 {bottom}");
        Ok(())
    }

    /// punch 通道:主包络 attack 拉慢时,单个响拍仍一拍把混合驱动值打到高位
    /// (punch 零 attack),随后按 `punch.release_ms` 快速衰减——「点」比主包络锐利。
    #[test]
    fn pulse_punch_spikes_on_transient() -> color_eyre::Result<()> {
        let overlay = |gain: f32| {
            serde_json::json!({
                "attack_ms": 300,
                "punch": { "gain": gain, "release_ms": 160 },
            })
        };
        let with_punch = pulse_cfg(&overlay(1.0))?;
        let without = pulse_cfg(&overlay(0.0))?;
        let loud = vec![0.5_f32; 256];
        let mut spiky = LoudnessPulse::new(/*tick_ms*/ 16);
        let mut smooth = LoudnessPulse::new(/*tick_ms*/ 16);
        spiky.feed(&loud, RATE, &with_punch);
        smooth.feed(&loud, RATE, &without);
        assert!(
            spiky.level_permille(&with_punch) > 900,
            "punch 应一拍打满,实得 {}",
            spiky.level_permille(&with_punch)
        );
        assert!(
            smooth.level_permille(&without) < 200,
            "无 punch 的慢 attack 主包络一拍只走一小步,实得 {}",
            smooth.level_permille(&without)
        );
        for _ in 0..40 {
            spiky.feed(&[], RATE, &with_punch);
        }
        assert!(
            spiky.level_permille(&with_punch) < 200,
            "静音后 punch 应快速衰减,实得 {}",
            spiky.level_permille(&with_punch)
        );
        Ok(())
    }

    /// gamma 感知曲线:同一「半程响度」时刻,gamma 3 的目标显著低于 gamma 1
    /// (中低响度被压低,鼓点间隙更沉)。
    #[test]
    fn pulse_gamma_darkens_midrange() -> color_eyre::Result<()> {
        let overlay = |gamma: f32| {
            serde_json::json!({
                "attack_ms": 0,
                "release_ms": 0,
                "gamma": gamma,
                "bass_cutoff_hz": 0.0,
            })
        };
        let linear = pulse_cfg_no_punch(overlay(1.0))?;
        let curved = pulse_cfg_no_punch(overlay(3.0))?;
        let full = vec![1.0_f32; 256];
        let half = vec![0.5_f32; 256];
        let mut a = LoudnessPulse::new(/*tick_ms*/ 16);
        let mut b = LoudnessPulse::new(/*tick_ms*/ 16);
        a.feed(&full, RATE, &linear);
        b.feed(&full, RATE, &curved);
        a.feed(&half, RATE, &linear);
        b.feed(&half, RATE, &curved);
        let (mid_linear, mid_curved) = (a.level_permille(&linear), b.level_permille(&curved));
        assert!(
            mid_linear > 400 && mid_linear < 600,
            "线性应近半程,实得 {mid_linear}"
        );
        assert!(
            mid_curved < mid_linear / 2,
            "gamma 3 应显著压低中程:{mid_curved} vs {mid_linear}"
        );
        Ok(())
    }

    /// 响度调制:满包络把场推得比零包络更浓(探针 cell 更接近场色);
    /// `pulse.enabled = false` 时同一包络完全不改变输出。
    #[test]
    fn pulse_deepens_field_and_disabled_ignores_level() -> color_eyre::Result<()> {
        let overlay = |enabled: bool| {
            serde_json::json!({
                "intensity": 0.4,
                "vignette": { "strength": 0.0 },
                "drift": { "sway_pct": 0.0 },
                "pulse": { "enabled": enabled, "depth": { "intensity": 0.3 } },
            })
        };
        let c = Rgb::new(220, 220, 220);
        let mut g = AmbientGradient::new(/*fade_ticks*/ 1, /*tick_ms*/ 16);
        let cfg = acfg(&overlay(true))?;
        g.set_target(Some(&palette(vec![c])?), BASE, cfg.anchors());
        g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        let area = Rect::new(0, 0, 8, 4);
        let probe = |cfg: &AmbientConfig, pulse: u16| -> color_eyre::Result<Color> {
            let mut buf = Buffer::empty(area);
            render(
                &mut buf, area, &g, BASE, cfg, /*progress_permille*/ 1000, pulse,
                /*skip*/ None,
            );
            bg_at(&buf, 4, 2)
        };
        let Color::Rgb(idle_r, ..) = probe(&cfg, /*pulse*/ 0)? else {
            return Err(eyre!("应为真彩"));
        };
        let Color::Rgb(loud_r, ..) = probe(&cfg, /*pulse*/ 1000)? else {
            return Err(eyre!("应为真彩"));
        };
        assert!(
            loud_r > idle_r,
            "满响度应比静默更浓(更接近场色):{loud_r} vs {idle_r}"
        );
        let disabled = acfg(&overlay(false))?;
        assert_eq!(
            probe(&disabled, /*pulse*/ 1000)?,
            probe(&disabled, /*pulse*/ 0)?,
            "关闭后包络不应影响输出"
        );
        Ok(())
    }

    /// 亮端推:满响度 + `depth.brightness = 1` 把所有锚点采样位推到色带最亮端,
    /// 全场即最亮色;静默时暗端锚点附近保持混合色。
    #[test]
    fn pulse_brightness_pushes_band_to_bright_end() -> color_eyre::Result<()> {
        let cfg = acfg(&serde_json::json!({
            "intensity": 1.0,
            "vignette": { "strength": 0.0 },
            "drift": { "sway_pct": 0.0 },
            "pulse": { "enabled": true, "depth": { "intensity": 0.0, "brightness": 1.0 } },
        }))?;
        let pal = blue_red()?;
        let bright = pal.sample_rgb(1000);
        let mut g = AmbientGradient::new(/*fade_ticks*/ 1, /*tick_ms*/ 16);
        g.set_target(Some(&pal), BASE, cfg.anchors());
        g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        let area = Rect::new(0, 0, 24, 8);
        let probe = |pulse: u16| -> color_eyre::Result<Color> {
            let mut buf = Buffer::empty(area);
            render(
                &mut buf, area, &g, BASE, &cfg, /*progress_permille*/ 1000, pulse,
                /*skip*/ None,
            );
            bg_at(&buf, 5, 1)
        };
        assert_eq!(
            probe(/*pulse*/ 1000)?,
            Color::Rgb(bright.r, bright.g, bright.b),
            "满推时全场应为色带最亮色"
        );
        assert_ne!(
            probe(/*pulse*/ 0)?,
            Color::Rgb(bright.r, bright.g, bright.b),
            "静默时暗端锚点附近不应是最亮色"
        );
        Ok(())
    }

    /// 色斑呼吸:`depth.sigma` 满响度时高斯半径放大,暗端锚点所在 cell 被更远的
    /// 亮端锚点掺进更多颜色(红分量上升)。
    #[test]
    fn pulse_sigma_swell_mixes_field() -> color_eyre::Result<()> {
        let cfg = acfg(&serde_json::json!({
            "intensity": 1.0,
            "vignette": { "strength": 0.0 },
            "drift": { "sway_pct": 0.0 },
            "pulse": { "enabled": true, "depth": { "intensity": 0.0, "sigma": 1.0 } },
        }))?;
        let mut g = AmbientGradient::new(/*fade_ticks*/ 1, /*tick_ms*/ 16);
        g.set_target(Some(&blue_red()?), BASE, cfg.anchors());
        g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        let area = Rect::new(0, 0, 24, 8);
        // (5,1) 即暗端锚点(0.22, 0.20)所在 cell:σ 膨胀 → 远处亮锚点权重上升。
        let probe = |pulse: u16| -> color_eyre::Result<Color> {
            let mut buf = Buffer::empty(area);
            render(
                &mut buf, area, &g, BASE, &cfg, /*progress_permille*/ 1000, pulse,
                /*skip*/ None,
            );
            bg_at(&buf, 5, 1)
        };
        let Color::Rgb(idle_r, ..) = probe(/*pulse*/ 0)? else {
            return Err(eyre!("应为真彩"));
        };
        let Color::Rgb(loud_r, ..) = probe(/*pulse*/ 1000)? else {
            return Err(eyre!("应为真彩"));
        };
        assert!(
            loud_r > idle_r,
            "σ 膨胀应让暗端 cell 掺入更多亮端色:{loud_r} vs {idle_r}"
        );
        Ok(())
    }

    /// 暗角开合:`depth.vignette` 满响度时暗角强度打满折,角落 cell 恰为场色
    /// (场涌到边缘);静默时暗角原样、角落偏向底色。
    #[test]
    fn pulse_vignette_opens_on_loud() -> color_eyre::Result<()> {
        let cfg = acfg(&serde_json::json!({
            "intensity": 1.0,
            "vignette": { "strength": 1.0 },
            "drift": { "sway_pct": 0.0 },
            "pulse": { "enabled": true, "depth": { "intensity": 0.0, "vignette": 1.0 } },
        }))?;
        let c = Rgb::new(200, 200, 40);
        let mut g = AmbientGradient::new(/*fade_ticks*/ 1, /*tick_ms*/ 16);
        g.set_target(Some(&palette(vec![c])?), BASE, cfg.anchors());
        g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        let area = Rect::new(0, 0, 20, 10);
        let probe = |pulse: u16| -> color_eyre::Result<Color> {
            let mut buf = Buffer::empty(area);
            render(
                &mut buf, area, &g, BASE, &cfg, /*progress_permille*/ 1000, pulse,
                /*skip*/ None,
            );
            bg_at(&buf, 0, 0)
        };
        assert_eq!(
            probe(/*pulse*/ 1000)?,
            Color::Rgb(c.r, c.g, c.b),
            "满响度暗角全开,角落应恰为场色"
        );
        assert_ne!(
            probe(/*pulse*/ 0)?,
            Color::Rgb(c.r, c.g, c.b),
            "静默时暗角原样,角落应偏向底色"
        );
        Ok(())
    }

    /// 遍历 area 内所有 cell 坐标。
    fn cells(area: Rect) -> Vec<(u16, u16)> {
        let mut out = Vec::<(u16, u16)>::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                out.push((x, y));
            }
        }
        out
    }

    /// 读某 cell 的 bg。
    fn bg_at(buf: &Buffer, x: u16, y: u16) -> color_eyre::Result<Color> {
        Ok(buf
            .cell((x, y))
            .ok_or_else(|| eyre!("cell ({x},{y}) 越界"))?
            .bg)
    }

    /// 把 buffer 内每 cell 的 bg 压成 4bit/通道 hex 网格文本(非真彩记 `---`)。
    fn bg_grid(buf: &Buffer, area: Rect) -> color_eyre::Result<String> {
        let mut out = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                match bg_at(buf, x, y)? {
                    Color::Rgb(r, g, b) => {
                        write!(out, "{:x}{:x}{:x} ", r >> 4, g >> 4, b >> 4)?;
                    }
                    _ => out.push_str("--- "),
                }
            }
            out.push('\n');
        }
        Ok(out)
    }
}
