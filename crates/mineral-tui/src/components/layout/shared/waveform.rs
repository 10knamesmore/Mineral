//! 进度条振幅波形:包络重采样到列宽 → 块字符字形 → 逐列着色 span 序列。
//!
//! 只做纯渲染零件;开关判定与包络归属校验在调用方(transport)。着色层次:
//! 已播(亮)> 已缓冲(中灰)> 未缓冲(暗灰);播放头不用异色块,而是在其前后
//! 软边窗口内让已播色与轨道色互相溶解(半径可配,0 = 硬边)。

use mineral_audio::Bps;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::components::layout::shared::transport::split_buffered_track;
use crate::render::anim::ease_out;
use crate::render::color::lerp_byte;
use crate::render::palette::{CoverPalette, column_permille};
use crate::render::theme::{Ink, Theme};
use crate::runtime::playback::EnvelopeState;
use crate::runtime::state::AppState;

/// 已播段取色策略:封面色板就绪时沿整条 bar 逐列渐变,否则单色。
pub enum PlayedStyle<'a> {
    /// 单色(`cover_color` 关 / 色板未就绪):与普通进度条已播段同色。
    Solid(Color),

    /// 逐列渐变:列位置经 `column_permille` 映射到色带采样(左暗右亮),
    /// 与频谱横轴同一空间语义。
    Gradient(&'a CoverPalette),
}

impl PlayedStyle<'_> {
    /// 第 `col` 列(共 `bar_w` 列)的已播颜色。
    ///
    /// 渐变锚定整条 bar 的列位置而非已播长度——列色恒定,播放推进只是逐列
    /// 揭开静态渐变,不会随进度整体变色。
    ///
    /// # Params:
    ///   - `col`: 列序号(从 0 起)
    ///   - `bar_w`: 进度条总列数
    ///
    /// # Return:
    ///   该列颜色。
    fn color_at(&self, col: usize, bar_w: usize) -> Color {
        match self {
            Self::Solid(color) => *color,
            Self::Gradient(palette) => palette.sample(column_permille(col, bar_w)),
        }
    }
}

/// 波形渲染上下文(逐帧从应用状态现读构造,不缓存配置)。
pub struct WaveformCtx<'a> {
    /// 总开关(`tui.waveform.enabled` 现读,overlay 翻转即热生效)。
    pub enabled: bool,

    /// 已播放段取色(封面色板渐变或主题 accent_2 单色,见 [`Self::new`])。
    pub played: PlayedStyle<'a>,

    /// 响度 → 条高的对比 gamma(`tui.waveform.contrast` 现读):1 = 线性。
    pub contrast: f32,

    /// 播放头软边半径(列,`tui.waveform.edge_radius` 现读):0 = 硬边。
    pub edge_radius: usize,

    /// 入场揭示动画参数(`tui.waveform.reveal.*` 现读)。
    pub reveal: RevealStyle,

    /// 当前曲包络及其入场动画相位(归属已校验);`None` = 未就绪,回落普通进度条。
    pub envelope: Option<&'a EnvelopeState>,
}

/// 波形入场揭示的形态参数(不含进度——进度在 [`EnvelopeState`] 上,随包络生灭)。
#[derive(Clone, Copy, Debug)]
pub struct RevealStyle {
    /// 横扫占总时长的比例(千分比 `0..=1000`,`tui.waveform.reveal.sweep_ratio` 现读):
    /// 余下为单列生长时长。满值 = 纯左→右擦除,`0` = 全条同时抬升。
    pub sweep: u16,

    /// 揭示边前沿的提亮强度(千分比 `0..=1000`,`tui.waveform.reveal.glow` 现读):
    /// `0` = 无亮边。
    pub glow: u16,
}

impl RevealStyle {
    /// 第 `col` 列(共 `bar_w` 列)在整体进度 `progress` 下的揭示进度。
    ///
    /// 各列按 `sweep` 比例错开起跑(左列先跑),每列在余下的时长内**各自** ease-out
    /// 长到位——两个方向合成一条对角线。
    ///
    /// # Params:
    ///   - `col`: 列序号(从 0 起)
    ///   - `bar_w`: 进度条总列数
    ///   - `progress`: 整体线性进度(千分比 `0..=1000`,见 [`EnvelopeState::reveal`])
    ///
    /// # Return:
    ///   该列揭示进度,千分比 `0..=1000`;`0` = 尚未揭示(渲染回落进度条中线)。
    fn column(&self, col: usize, bar_w: usize, progress: u16) -> u16 {
        let sweep = self.sweep.min(FULL_E3);
        // 起跑时刻:末列恰在 `sweep` 处起跑,故整条在 `sweep + grow = 满值` 时收尾。
        let start = match u16::try_from(bar_w.saturating_sub(1)) {
            Ok(0) | Err(_) => 0,
            Ok(last) => {
                u16::try_from(u32::from(sweep) * u32::try_from(col).unwrap_or(0) / u32::from(last))
                    .unwrap_or(sweep)
            }
        };
        let Some(elapsed) = progress.checked_sub(start).filter(|e| *e > 0) else {
            return 0;
        };
        let grow = FULL_E3 - sweep;
        if grow == 0 {
            return FULL_E3; // 纯擦除:起跑即到位
        }
        ease_out(
            u16::try_from(u32::from(elapsed) * u32::from(FULL_E3) / u32::from(grow))
                .unwrap_or(FULL_E3)
                .min(FULL_E3),
        )
    }
}

/// 千分比满值(与 [`crate::render::anim`] 同一标度)。
const FULL_E3: u16 = 1000;

impl<'a> WaveformCtx<'a> {
    /// 从应用状态现读构造:包络经 [`crate::runtime::playback::Playback::current_envelope`]
    /// 归属校验;已播段在 `cover_color` 开且当前曲封面取色就绪时沿整条色带逐列渐变
    /// (与频谱同一份 kmeans 产物、同一空间语义),否则回落主题 accent_2 单色。
    ///
    /// 色板取自 [`crate::image::ImageEngine::current_palette`](随封面身份变化的
    /// 稳定拷贝)而非原图 LRU 派生的 `palettes`——后者被 browse 滚动 churn 逐出又重取,
    /// 直接读会让渐变在 Gradient↔Solid 间闪烁。
    ///
    /// # Params:
    ///   - `state`: 应用状态
    ///   - `theme`: 取色主题
    ///
    /// # Return:
    ///   当帧波形上下文。
    pub fn new(state: &'a AppState, theme: &Theme) -> Self {
        let cfg = state.cfg.tui().waveform();
        let played = if *cfg.cover_color() {
            state
                .images
                .current_palette
                .as_ref()
                .map_or(PlayedStyle::Solid(theme.accent_2), PlayedStyle::Gradient)
        } else {
            PlayedStyle::Solid(theme.accent_2)
        };
        Self {
            enabled: *cfg.enabled(),
            played,
            contrast: *cfg.contrast(),
            edge_radius: *cfg.edge_radius(),
            reveal: RevealStyle {
                sweep: ratio_e3(*cfg.reveal().sweep_ratio()),
                glow: ratio_e3(*cfg.reveal().glow()),
            },
            envelope: state.playback.current_envelope(),
        }
    }
}

/// 配置里的 `0.0..=1.0` 比例 → 千分比定点(超界 clamp,NaN 归零)。
///
/// # Params:
///   - `ratio`: 配置比例
///
/// # Return:
///   千分比,`0..=1000`。
#[allow(clippy::as_conversions)] // reason: 浮点比例到定点,值域已 clamp 进 0..=1000
fn ratio_e3(ratio: f32) -> u16 {
    if !ratio.is_finite() {
        return 0;
    }
    (ratio * 1000.0).round().clamp(0.0, 1000.0) as u16
}

/// 响度 → 条高的对比 gamma 映射:`(v/255)^contrast × 255`,端点不动、单调。
///
/// 渲染层映射不改包络数据——旋钮热更即时生效,不触发任何重算。
///
/// # Params:
///   - `height`: 归一响度(0..=255)
///   - `contrast`: gamma 指数(1 = 线性;调用方来自配置,非正值按 1 处理)
///
/// # Return:
///   映射后的高度(0..=255)。
#[allow(clippy::as_conversions)] // reason: 浮点幂映射到 u8,值域已 clamp 进 0..=255
pub(crate) fn apply_contrast(height: u8, contrast: f32) -> u8 {
    if !(contrast.is_finite() && contrast > 0.0) || (contrast - 1.0).abs() < f32::EPSILON {
        return height;
    }
    let normalized = f32::from(height) / 255.0;
    (normalized.powf(contrast) * 255.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// 8 级下块字符阶梯(振幅高度 → 字形)。
const LADDER: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// 振幅(0..=255)→ 块字形:线性量化到 8 级。
///
/// # Params:
///   - `height`: 振幅高度
///
/// # Return:
///   对应的下块字符(恒 1 cell 宽)。
pub(crate) fn glyph_for(height: u8) -> &'static str {
    let index = usize::from(height) * LADDER.len() / 256;
    LADDER.get(index).copied().unwrap_or("█")
}

/// 包络点重采样到目标列数:缩小按桶取峰(不丢突刺),放大整数定点线性插值。
///
/// # Params:
///   - `points`: 包络点(0..=255)
///   - `columns`: 目标列数
///
/// # Return:
///   定长 `columns` 的高度序列;任一输入为零得空。
pub(crate) fn resample_columns(points: &[u8], columns: usize) -> Vec<u8> {
    let m = points.len();
    if m == 0 || columns == 0 {
        return Vec::new();
    }
    if m >= columns {
        (0..columns)
            .map(|i| {
                let lo = i * m / columns;
                let hi = ((i + 1) * m / columns).max(lo + 1).min(m);
                points
                    .get(lo..hi)
                    .unwrap_or_default()
                    .iter()
                    .copied()
                    .max()
                    .unwrap_or(0)
            })
            .collect()
    } else {
        // 放大:256 分度定点插值(输入本就是 u8 粗粒度,整数运算足够)。
        (0..columns)
            .map(|i| {
                if m == 1 {
                    return points.first().copied().unwrap_or(0);
                }
                let position = i * (m - 1) * 256 / (columns - 1);
                let lo = (position / 256).min(m - 1);
                let frac = position % 256;
                let a = points.get(lo).copied().unwrap_or(0);
                let b = points.get(lo + 1).copied().unwrap_or(a);
                let value = (usize::from(a) * (256 - frac) + usize::from(b) * frac) / 256;
                u8::try_from(value).unwrap_or(u8::MAX)
            })
            .collect()
    }
}

/// 两色按 `num/denom` 逐分量插值;任一非 RGB(ANSI 主题色拆不出分量)时按中点
/// 硬切,与 hue 旋转对非 RGB 色的兜底同一态度。
///
/// # Params:
///   - `a` / `b`: 两端色
///   - `num` / `denom`: 插值比例(0 = 全 `a`,`denom` = 全 `b`)
///
/// # Return:
///   插值色。
fn mix_colors(a: Color, b: Color, num: u64, denom: u64) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => Color::Rgb(
            lerp_byte(r1, r2, num, denom),
            lerp_byte(g1, g2, num, denom),
            lerp_byte(b1, b2, num, denom),
        ),
        _ => {
            if num.saturating_mul(2) < denom {
                a
            } else {
                b
            }
        }
    }
}

/// 波形渲染需要的播放位置口径。
#[derive(Clone, Copy, Debug)]
pub struct PlayState {
    /// 已播比例(播放头连续位置;整列数与软边比例都由它派生)。
    pub progress: Bps,

    /// 已缓冲比例(播放头之后拆亮 / 暗两段,与普通进度条同语义)。
    pub buffered: Bps,
}

/// 未揭示列的进度条形态 cell:`━`(已播)/ `●`(播放头)/ `─`(轨道,已缓冲亮、未缓冲暗)。
///
/// 与 transport 回落分支的普通进度条**逐 cell 一致**——入场动画因此读作「波形把中线
/// 从左往右推走」的形态转换,而不是波形凭空出现。
///
/// # Params:
///   - `col`: 列序号
///   - `filled`: 已播实心列数(播放头在第 `filled` 列)
///   - `bright_end`: 轨道亮暗分界列
///   - `theme`: 取色主题
///   - `ink`: 对实际背景现算的弱化色阶
///
/// # Return:
///   `(字形, 样式)`。
fn plain_cell(
    col: usize,
    filled: usize,
    bright_end: usize,
    theme: &Theme,
    ink: Ink,
) -> (&'static str, Style) {
    if col < filled {
        ("━", Style::new().fg(theme.accent_2))
    } else if col == filled {
        (
            "●",
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        )
    } else if col < bright_end {
        ("─", Style::new().fg(ink.muted))
    } else {
        ("─", Style::new().fg(ink.ghost))
    }
}

/// 揭示边前沿提亮:正在生长的列朝 `theme.text` 混色,越接近到位越回落本色。
///
/// `column == 满值`(已到位)时混合比恒为 0,故动画放完那一帧与无动画时逐 cell 一致。
///
/// # Params:
///   - `color`: 该列本色
///   - `glow`: 提亮强度(千分比)
///   - `column`: 该列揭示进度(千分比)
///   - `theme`: 取色主题
///
/// # Return:
///   提亮后的颜色。
fn glow_mix(color: Color, glow: u16, column: u16, theme: &Theme) -> Color {
    let num = u64::from(FULL_E3.saturating_sub(column)) * u64::from(glow) / u64::from(FULL_E3);
    mix_colors(color, theme.text, num, u64::from(FULL_E3))
}

/// 波形进度条 span 序列(总 cell 宽恒 == `bar_w`,布局不抖)。
///
/// 播放头不画异色块:`edge_radius > 0` 时,播放头前后各 `edge_radius` 列在已播色
/// 与轨道色之间线性插值,边界溶解;`0` = 硬边(已播渐变的生长边缘即 seek 位置)。
/// 插值比例以**亚列精度**(万分之一列定点)跟随播放位置——整列量化会让整个
/// 软边窗口随播放推进一格一格跳变,而不是连续滑过。
///
/// 包络刚到达时整条按 [`RevealStyle`] 逐列揭示:未揭示列走 [`plain_cell`] 维持进度条
/// 中线,已揭示列按该列进度从底部长到目标高度、前沿由 [`glow_mix`] 提亮。揭示满值后
/// 这两层都退化为恒等,渲染回到纯稳态。
///
/// # Params:
///   - `env`: 当前曲包络及其揭示进度(归属校验在调用方)
///   - `bar_w`: 目标列数
///   - `play`: 播放位置口径
///   - `wave`: 波形样式(取色 / 对比 / 软边 / 揭示形态)
///   - `theme`: 取色主题
///   - `ink`: 对实际背景现算的弱化色阶(未揭示段的轨道色)
///
/// # Return:
///   按样式分段合并后的 span 序列。
pub(crate) fn waveform_spans(
    env: &EnvelopeState,
    bar_w: usize,
    play: PlayState,
    wave: &WaveformCtx<'_>,
    theme: &Theme,
    ink: Ink,
) -> Vec<Span<'static>> {
    if bar_w == 0 {
        return Vec::new();
    }
    let heights = resample_columns(&env.envelope().points, bar_w)
        .iter()
        .map(|&h| apply_contrast(h, wave.contrast))
        .collect::<Vec<u8>>();
    let reveal = env.reveal();
    let filled = play.progress.of(bar_w).min(bar_w);
    // 播放头连续位置(万分之一列定点):亚列偏移进入软边插值比例。
    let head_e4 = u64::from(play.progress.get()).saturating_mul(u64::try_from(bar_w).unwrap_or(0));
    // 轨道亮暗边界(已缓冲 overlay / 未缓冲 surface0);播完则无轨道。
    let bright_end = if filled < bar_w {
        let (bright, _) = split_buffered_track(bar_w, filled, play.buffered);
        filled + 1 + bright
    } else {
        bar_w
    };
    let column_e4 = |col: usize| u64::try_from(col).unwrap_or(0).saturating_mul(10_000);
    let radius_e4 = u64::try_from(wave.edge_radius)
        .unwrap_or(0)
        .saturating_mul(10_000);
    let color_at = |col: usize| -> Color {
        let played_color = wave.played.color_at(col, bar_w);
        if filled >= bar_w {
            return played_color;
        }
        let track_color = if col < bright_end {
            theme.overlay
        } else {
            theme.surface0
        };
        if wave.edge_radius == 0 {
            // 硬边:播放头列并入已播渐变(渐变生长边缘即 seek 位置)。
            return if col <= filled {
                played_color
            } else {
                track_color
            };
        }
        let col_e4 = column_e4(col);
        if col_e4.saturating_add(radius_e4) < head_e4 {
            return played_color;
        }
        if col_e4 > head_e4.saturating_add(radius_e4) {
            return track_color;
        }
        // 软边窗口 [head-r, head+r](连续):比例 = (col + r - head) / 2r,
        // 播放头随 progress 每 ‱ 的推进都让窗口内列色平滑滑动。
        let num = col_e4.saturating_add(radius_e4).saturating_sub(head_e4);
        mix_colors(played_color, track_color, num, radius_e4.saturating_mul(2))
    };
    let cell = |col: usize| -> (&'static str, Style) {
        let column = wave.reveal.column(col, bar_w, reveal);
        if column == 0 {
            return plain_cell(col, filled, bright_end, theme, ink);
        }
        // 生长:目标高度按该列进度缩放,再由 `glyph_for` 量化回 8 级阶梯。
        let height = u8::try_from(
            u32::from(heights.get(col).copied().unwrap_or(0)) * u32::from(column)
                / u32::from(FULL_E3),
        )
        .unwrap_or(u8::MAX);
        let color = glow_mix(color_at(col), wave.reveal.glow, column, theme);
        (glyph_for(height), Style::new().fg(color))
    };
    // 逐列取样式,相邻同样式列合并成一个 span(远离播放头 / 揭示边的纯色区自然收敛成长 run)。
    let mut spans = Vec::new();
    let mut run = String::new();
    let (_, mut run_style) = cell(0);
    for col in 0..bar_w {
        let (glyph, style) = cell(col);
        if col > 0 && style != run_style {
            spans.push(Span::styled(std::mem::take(&mut run), run_style));
            run_style = style;
        }
        run.push_str(glyph);
    }
    spans.push(Span::styled(run, run_style));
    spans
}

#[cfg(test)]
mod tests {

    use super::resample_columns;

    /// `ratio_e3`:常规比例四舍五入到千分比,超界 clamp,非有限值(NaN / inf)归零而非炸。
    #[test]
    fn ratio_e3_clamps_and_rejects_non_finite() {
        assert_eq!(super::ratio_e3(0.62), 620);
        assert_eq!(super::ratio_e3(0.0), 0);
        assert_eq!(super::ratio_e3(1.0), 1000);
        assert_eq!(super::ratio_e3(-0.5), 0, "负值 clamp 到 0");
        assert_eq!(super::ratio_e3(3.0), 1000, "超界 clamp 到满值");
        assert_eq!(super::ratio_e3(f32::NAN), 0, "NaN 归零");
        assert_eq!(super::ratio_e3(f32::INFINITY), 0, "inf 归零");
    }

    /// 重采样定长:缩小(200→10)与放大(3→10)都恰好得到目标列数;空输入得空。
    #[test]
    fn resample_matches_column_count() {
        assert_eq!(resample_columns(&[128u8; 200], 10).len(), 10);
        assert_eq!(resample_columns(&[0, 128, 255], 10).len(), 10);
        assert_eq!(resample_columns(&[], 10), Vec::<u8>::new());
        assert_eq!(resample_columns(&[128u8; 200], 0), Vec::<u8>::new());
    }

    /// 缩小按桶取峰:单点突刺(255)不被平均抹掉,落点列必是满值。
    #[test]
    fn downsample_keeps_transient_peak() {
        let mut points = vec![10u8; 100];
        if let Some(spike) = points.get_mut(55) {
            *spike = 255;
        }
        let columns = resample_columns(&points, 10);
        assert_eq!(columns.get(5).copied(), Some(255), "突刺应保留在第 6 列");
        assert!(
            columns.iter().filter(|&&c| c == 255).count() == 1,
            "只有突刺列是满值:{columns:?}"
        );
    }
}
