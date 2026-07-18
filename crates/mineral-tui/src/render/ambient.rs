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

use mineral_config::{AmbientConfig, AnchorConfig};
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
        self.from = Some(self.anchor_colors(base, anchors));
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
    /// 锚点 Lab 插值。终点 = 色板在「锚点采样位经轮转相位映射」处的取色(随轮转流动),
    /// `None` 时现读底色(渐变途中热更主题即追新底色);起点缺位(锚点表热更变长)补底色。
    fn anchor_colors(&self, base: Rgb, anchors: &[AnchorConfig]) -> Vec<Rgb> {
        let end_at = |anchor: &AnchorConfig| -> Rgb {
            self.to.as_ref().map_or(base, |palette| {
                palette.sample_rgb(rotated_pos(*anchor.pos(), self.rotate_phase))
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
///   - `skip`: 不铺的洞(将被不透明终端图协议真图盖住的封面区)。图协议把整段载荷
///     藏在图区首 cell 的 symbol 里,逐帧改那格 bg 会让 diff 每帧重发载荷——
///     iTerm2 / sixel(数据即显示、自带擦行)表现为整图闪烁;图不透明,跳过零视觉损失
pub fn render(
    buf: &mut Buffer,
    area: Rect,
    gradient: &AmbientGradient,
    base: Rgb,
    cfg: &AmbientConfig,
    progress_permille: u16,
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
    let colors = gradient.anchor_colors(base, anchors);
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
    let intensity =
        (cfg.intensity() * f32::from(progress_permille.min(1000)) / 1000.0).clamp(0.0, 1.0);
    let vignette = cfg.vignette();
    let (veil_strength, veil_inner) = (vignette.strength().clamp(0.0, 1.0), *vignette.inner());
    // 满强半径贴着起始半径也不除零:压出一段极窄的过渡带。
    let veil_span = (vignette.outer() - veil_inner).max(1e-3);
    let sigma = cfg.sigma().max(1e-3);
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

    use super::{AmbientGradient, render, rgb_of, rotated_pos};
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
            g.anchor_colors(BASE, cfg.anchors()),
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
            g.anchor_colors(BASE, cfg.anchors()),
            vec![BASE; cfg.anchors().len()],
            "frame 0 应从底色起步"
        );
        for _ in 0..5 {
            g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        }
        let mid = g.anchor_colors(BASE, cfg.anchors());
        let band = expected_band(&pal, &cfg);
        for (got, end) in mid.into_iter().zip(band.iter().copied()) {
            assert_eq!(got, lerp_lab(BASE, end, 500), "中点应是 Lab 半程插值");
        }
        for _ in 0..5 {
            g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        }
        assert_eq!(
            g.anchor_colors(BASE, cfg.anchors()),
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
        let before = g.anchor_colors(BASE, cfg.anchors());
        let green = palette(vec![Rgb::new(20, 120, 20)])?;
        g.set_target(Some(&green), BASE, cfg.anchors());
        assert_eq!(
            g.anchor_colors(BASE, cfg.anchors()),
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
            g.anchor_colors(BASE, cfg.anchors()),
            vec![BASE; n],
            "回落起点是封面色,不瞬跳"
        );
        for _ in 0..10 {
            g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        }
        assert_eq!(g.anchor_colors(BASE, cfg.anchors()), vec![BASE; n]);
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
        let before = g.anchor_colors(BASE, cfg.anchors());
        g.retempo(/*fade_ticks*/ 20, /*tick_ms*/ 16);
        assert_eq!(
            g.anchor_colors(BASE, cfg.anchors()),
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
        let before = g.anchor_colors(BASE, cfg.anchors());
        g.set_target(Some(&pal), BASE, cfg.anchors());
        assert_eq!(
            g.anchor_colors(BASE, cfg.anchors()),
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
        let before = g.anchor_colors(BASE, cfg.anchors());
        let mut frozen = g.clone();
        // 周期 4s、拍长 16ms:125 拍走完半圈,采样位翻转到对侧,颜色必然变化。
        for _ in 0..125 {
            g.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 4.0);
            frozen.tick(/*drift_speed*/ 0.0, /*rotate_cycle_secs*/ 0.0);
        }
        assert_ne!(
            g.anchor_colors(BASE, cfg.anchors()),
            before,
            "轮转推进后锚点色应流动"
        );
        assert_eq!(
            frozen.anchor_colors(BASE, cfg.anchors()),
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
            &mut buf, area, &g, BASE, &cfg, /*progress_permille*/ 1000, /*skip*/ None,
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
            &mut buf, area, &g, BASE, &cfg, /*progress_permille*/ 1000, /*skip*/ None,
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
            &mut buf, area, &g, BASE, &cfg, /*progress_permille*/ 1000, /*skip*/ None,
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
            &mut buf, area, &g, BASE, &cfg, /*progress_permille*/ 1000, /*skip*/ None,
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
                &mut buf, area, g, BASE, &cfg, /*progress_permille*/ 1000, /*skip*/ None,
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
            render(&mut buf, area, &g, BASE, &cfg, permille, /*skip*/ None);
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
            &mut buf, area, &g, BASE, &cfg, /*progress_permille*/ 1000, /*skip*/ None,
        );
        let grid = bg_grid(&buf, area)?;
        mineral_test::assert_snap!(
            "氛围渐变场:三色板稳态 24×8,默认观感配置(bg 压 4bit hex)",
            grid
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
