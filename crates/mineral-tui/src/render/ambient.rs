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

use crate::image::colors::lerp_lab;
use crate::render::palette::{CoverPalette, Rgb};

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
