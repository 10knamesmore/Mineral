//! 进度条振幅波形:包络重采样到列宽 → 块字符字形 → 逐列着色 span 序列。
//!
//! 只做纯渲染零件;开关判定与包络归属校验在调用方(transport)。着色层次:
//! 已播(亮)> 已缓冲(中灰)> 未缓冲(暗灰);播放头不用异色块,而是在其前后
//! 软边窗口内让已播色与轨道色互相溶解(半径可配,0 = 硬边)。

use mineral_audio::Bps;
use mineral_model::Envelope;
use ratatui::style::{Color, Style};
use ratatui::text::Span;

use crate::components::layout::shared::transport::split_buffered_track;
use crate::render::color::lerp_byte;
use crate::render::palette::{CoverPalette, column_permille};
use crate::render::theme::Theme;
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

    /// 当前曲包络(归属已校验);`None` = 未就绪,回落普通进度条。
    pub envelope: Option<&'a Envelope>,
}

impl<'a> WaveformCtx<'a> {
    /// 从应用状态现读构造:包络经 [`crate::runtime::playback::Playback::current_envelope`]
    /// 归属校验;已播段在 `cover_color` 开且当前曲封面取色就绪时沿整条色带逐列渐变
    /// (与频谱同一份 kmeans 产物、同一空间语义),否则回落主题 accent_2 单色。
    ///
    /// 色板取自 [`crate::runtime::state::covers::CoverHub::current_palette`](随封面身份变化的
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
                .covers
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
            envelope: state.playback.current_envelope(),
        }
    }

    /// 关闭态上下文(测试用):恒回落普通进度条。
    #[cfg(test)]
    pub fn off() -> WaveformCtx<'static> {
        WaveformCtx {
            enabled: false,
            played: PlayedStyle::Solid(Color::Reset),
            contrast: 1.0,
            edge_radius: 0,
            envelope: None,
        }
    }
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

/// 波形进度条 span 序列(总 cell 宽恒 == `bar_w`,布局不抖)。
///
/// 播放头不画异色块:`edge_radius > 0` 时,播放头前后各 `edge_radius` 列在已播色
/// 与轨道色之间线性插值,边界溶解;`0` = 硬边(已播渐变的生长边缘即 seek 位置)。
/// 插值比例以**亚列精度**(万分之一列定点)跟随播放位置——整列量化会让整个
/// 软边窗口随播放推进一格一格跳变,而不是连续滑过。
///
/// # Params:
///   - `points`: 包络点(0..=255)
///   - `bar_w`: 目标列数
///   - `progress`: 已播比例(播放头连续位置;整列数与软边比例都由它派生)
///   - `buffered`: 已缓冲比例(播放头之后拆亮 / 暗两段,与普通进度条同语义)
///   - `played`: 已播段取色(单色或沿色带逐列渐变)
///   - `contrast`: 对比 gamma(重采样后逐列映射;1 = 线性)
///   - `edge_radius`: 播放头软边半径(列)
///   - `theme`: 取色主题
///
/// # Return:
///   按颜色分段合并后的 span 序列。
#[allow(clippy::too_many_arguments)] // reason: 纯渲染零件,参数即全部输入,收拢成 struct 反而多一层搬运
pub(crate) fn waveform_spans(
    points: &[u8],
    bar_w: usize,
    progress: Bps,
    buffered: Bps,
    played: &PlayedStyle<'_>,
    contrast: f32,
    edge_radius: usize,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let glyphs = resample_columns(points, bar_w)
        .iter()
        .map(|&h| glyph_for(apply_contrast(h, contrast)))
        .collect::<Vec<&'static str>>();
    let filled = progress.of(bar_w).min(bar_w);
    // 播放头连续位置(万分之一列定点):亚列偏移进入软边插值比例。
    let head_e4 = u64::from(progress.get()).saturating_mul(u64::try_from(bar_w).unwrap_or(0));
    // 轨道亮暗边界(已缓冲 overlay / 未缓冲 surface0);播完则无轨道。
    let bright_end = if filled < bar_w {
        let (bright, _) = split_buffered_track(bar_w, filled, buffered);
        filled + 1 + bright
    } else {
        bar_w
    };
    let column_e4 = |col: usize| u64::try_from(col).unwrap_or(0).saturating_mul(10_000);
    let radius_e4 = u64::try_from(edge_radius)
        .unwrap_or(0)
        .saturating_mul(10_000);
    let color_at = |col: usize| -> Color {
        let played_color = played.color_at(col, bar_w);
        if filled >= bar_w {
            return played_color;
        }
        let track_color = if col < bright_end {
            theme.overlay
        } else {
            theme.surface0
        };
        if edge_radius == 0 {
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
    let run = |range: std::ops::Range<usize>| -> String {
        glyphs.get(range).unwrap_or_default().concat()
    };
    // 逐列取色,相邻同色列合并成一个 span(远离播放头的纯色区自然收敛成长 run)。
    let mut spans = Vec::new();
    if bar_w == 0 {
        return spans;
    }
    let mut run_start = 0usize;
    let mut run_color = color_at(0);
    for col in 1..bar_w {
        let color = color_at(col);
        if color != run_color {
            spans.push(Span::styled(
                run(run_start..col),
                Style::new().fg(run_color),
            ));
            run_start = col;
            run_color = color;
        }
    }
    spans.push(Span::styled(
        run(run_start..bar_w),
        Style::new().fg(run_color),
    ));
    spans
}

#[cfg(test)]
mod tests {
    use mineral_audio::Bps;
    use ratatui::style::Color;
    use ratatui::text::Span;

    use super::{PlayedStyle, glyph_for, resample_columns, waveform_spans};
    use crate::render::palette::{CoverPalette, Rgb, column_permille};
    use crate::render::theme::Theme;

    /// 把 span 序列展开成逐 cell 的 `(字符, 前景色)`(块字符均为 1 cell 宽)。
    fn cells(spans: &[Span<'_>]) -> Vec<(char, Color)> {
        spans
            .iter()
            .flat_map(|s| {
                let fg = s.style.fg.unwrap_or(Color::Reset);
                s.content
                    .chars()
                    .map(move |c| (c, fg))
                    .collect::<Vec<(char, Color)>>()
            })
            .collect()
    }

    /// contrast = 1 是恒等映射:与不加映射逐点相等(线性基线可显式关闭 gamma)。
    #[test]
    fn contrast_one_is_identity() {
        for h in [0u8, 1, 64, 128, 200, 255] {
            assert_eq!(super::apply_contrast(h, 1.0), h, "gamma 1 必须恒等: h={h}");
        }
    }

    /// contrast > 1 压低中段、端点不动且保持单调:0/255 是不动点,
    /// 中间值下沉(gamma 2 下 128 ≈ 0.5² × 255 ≈ 64),次序不得翻转。
    #[test]
    fn contrast_darkens_midtones_and_keeps_order() {
        assert_eq!(super::apply_contrast(0, 2.0), 0);
        assert_eq!(super::apply_contrast(255, 2.0), 255);
        assert_eq!(super::apply_contrast(128, 2.0), 64);
        let mut prev = 0u8;
        for h in 0..=255u8 {
            let mapped = super::apply_contrast(h, 2.0);
            assert!(mapped >= prev, "gamma 映射必须单调: h={h}");
            assert!(mapped <= h, "gamma > 1 不得抬升任何点: h={h}");
            prev = mapped;
        }
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

    /// 高度→块字形:0 最矮(▁)、255 满块(█)、随高度单调不降。
    #[test]
    fn glyph_levels_monotonic() -> color_eyre::Result<()> {
        assert_eq!(glyph_for(0), "▁");
        assert_eq!(glyph_for(255), "█");
        const LADDER: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
        let rank = |g: &str| LADDER.iter().position(|l| *l == g);
        let mut prev = 0usize;
        for h in 0..=255u8 {
            let r = rank(glyph_for(h))
                .ok_or_else(|| color_eyre::eyre::eyre!("字形必须落在 8 级阶梯内: h={h}"))?;
            assert!(r >= prev, "字形高度须随振幅单调不降: h={h}");
            prev = r;
        }
        Ok(())
    }

    /// 渐变已播段:逐列颜色 == 色板沿整条 bar 的列位置采样(与频谱同一空间语义),
    /// 且列色只随列位置定、不随 filled 变——播放推进只是逐列揭开静态渐变,不整体变色。
    #[test]
    fn gradient_played_colors_follow_palette() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let palette = CoverPalette::new(vec![
            Rgb::new(20, 10, 60),
            Rgb::new(120, 60, 160),
            Rgb::new(250, 200, 80),
        ])
        .ok_or_else(|| color_eyre::eyre::eyre!("非空应构造成功"))?;
        let points = vec![128u8; 200];
        let bar_w = 40usize;
        let render = |progress: Bps| -> Vec<(char, Color)> {
            cells(&waveform_spans(
                &points,
                bar_w,
                progress,
                Bps::FULL,
                &PlayedStyle::Gradient(&palette),
                /*contrast*/ 1.0,
                /*edge_radius*/ 0,
                &theme,
            ))
        };
        // 5000‱ × 40 列 = 已播 20 列。
        let cells_at = render(Bps::new(5_000));
        for (col, (_, fg)) in cells_at.iter().take(20).enumerate() {
            // edge_radius 0 = 硬边:已播列全是色带采样,无混色。
            assert_eq!(
                *fg,
                palette.sample(column_permille(col, bar_w)),
                "第 {col} 列已播色应是色带上该列位置的采样"
            );
        }
        // 列色锚定列位置:推进 filled 后,先前已播列颜色不变。
        let advanced = render(Bps::new(7_500));
        assert_eq!(
            cells_at.get(..20).map(<[(char, Color)]>::to_vec),
            advanced.get(..20).map(<[(char, Color)]>::to_vec),
            "播放推进不得改变既有已播列的颜色"
        );
        Ok(())
    }

    /// span 序列 cell 宽守恒 == bar_w;远离播放头处纯已播色 / 纯轨道色(overlay 在前
    /// surface0 在后不交错),混色只出现在播放头软边窗口内,且**无任何白块列**
    /// (theme.text)——播放头指示完全由颜色过渡承担。
    #[test]
    fn spans_conserve_width_and_partition_colors() {
        let theme = Theme::default();
        let played = Color::Rgb(226, 184, 107);
        let points = vec![128u8; 200];
        let bar_w = 40usize;
        let filled = 10usize;
        let radius = 3usize;
        let spans = waveform_spans(
            &points,
            bar_w,
            Bps::new(2_500), // 2500‱ × 40 = 已播 10 列
            Bps::new(5_000),
            &PlayedStyle::Solid(played),
            /*contrast*/ 1.0,
            /*edge_radius*/ radius,
            &theme,
        );
        let cells = cells(&spans);
        assert_eq!(cells.len(), bar_w, "波形总宽必须恰等于 bar_w,布局不抖");
        assert!(
            cells.iter().all(|(_, c)| *c != theme.text),
            "不得再出现白块播放头列"
        );
        // 软边窗口之前:纯已播色。
        assert!(
            cells
                .iter()
                .take(filled - radius)
                .all(|(_, c)| *c == played),
            "软边窗口之前应纯已播色"
        );
        // 软边窗口之后:纯轨道色,overlay 全在 surface0 之前。
        let track = cells
            .iter()
            .skip(filled + radius + 1)
            .collect::<Vec<&(char, Color)>>();
        assert!(
            track
                .iter()
                .all(|(_, c)| *c == theme.overlay || *c == theme.surface0),
            "软边窗口之后只该有 overlay/surface0 两色"
        );
        let last_bright = track.iter().rposition(|(_, c)| *c == theme.overlay);
        let first_dim = track.iter().position(|(_, c)| *c == theme.surface0);
        if let (Some(lb), Some(fd)) = (last_bright, first_dim) {
            assert!(lb < fd, "已缓冲亮段应全部在未缓冲暗段之前");
        }
        // 混色(既非已播色也非轨道色)只出现在软边窗口内。
        for (col, (_, c)) in cells.iter().enumerate() {
            if *c != played && *c != theme.overlay && *c != theme.surface0 {
                assert!(
                    col + radius >= filled && col <= filled + radius,
                    "混色列 {col} 落在软边窗口 [{}, {}] 之外",
                    filled - radius,
                    filled + radius
                );
            }
        }
    }

    /// 软边混色精确性:窗口中心(播放头列)是两端色的等比中点,窗口两端逐渐
    /// 收敛到纯色——用与实现同一把 mix 尺子对照,锁死插值比例。
    #[test]
    fn soft_edge_blends_between_played_and_track() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let played = Color::Rgb(200, 0, 0);
        let points = vec![255u8; 200];
        let bar_w = 40usize;
        let filled = 20usize;
        let radius = 3usize;
        let cells = cells(&waveform_spans(
            &points,
            bar_w,
            Bps::new(5_000), // 恰在整列上(20.0 列):窗口中心即等比中点
            Bps::FULL,       // 满缓冲:轨道全 overlay,不引入亮暗边界干扰
            &PlayedStyle::Solid(played),
            /*contrast*/ 1.0,
            /*edge_radius*/ radius,
            &theme,
        ));
        for offset in 0..=(2 * radius) {
            let col = filled - radius + offset;
            let expected = super::mix_colors(
                played,
                theme.overlay,
                u64::try_from(offset)?,
                u64::try_from(2 * radius)?,
            );
            assert_eq!(
                cells.get(col).map(|(_, c)| *c),
                Some(expected),
                "软边窗口第 {offset} 列插值比例应为 {offset}/{}",
                2 * radius
            );
        }
        Ok(())
    }

    /// 播放头亚列精度:seek 位置在同一整列内滑动(整列数不变)时,软边混色比例
    /// 必须跟着连续变化——列级量化会让整个窗口一格一格跳变。
    #[test]
    fn sub_column_seek_shifts_blend_smoothly() {
        let theme = Theme::default();
        let played = Color::Rgb(200, 0, 0);
        let points = vec![255u8; 200];
        let bar_w = 40usize;
        let at = |bps: u16| -> Vec<(char, Color)> {
            cells(&waveform_spans(
                &points,
                bar_w,
                Bps::new(bps),
                Bps::FULL,
                &PlayedStyle::Solid(played),
                /*contrast*/ 1.0,
                /*edge_radius*/ 3,
                &theme,
            ))
        };
        // 5000‱ × 40 = 整 20 列;5125‱ × 40 = 20.5 列——同一整列,亚列偏移半格。
        let on_column = at(5_000);
        let half_column = at(5_125);
        assert_ne!(on_column, half_column, "亚列 seek 位移必须反映到软边混色");
        // 平滑性:每 5‱(0.02 列)步进,任何列的颜色分量变化都有界——不整格跳。
        let mut prev = at(5_000);
        for bps in (5_005..=5_200).step_by(5) {
            let next = at(bps);
            for (a, b) in prev.iter().zip(next.iter()) {
                if let (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) = (a.1, b.1) {
                    let delta = r1.abs_diff(r2).max(g1.abs_diff(g2)).max(b1.abs_diff(b2));
                    assert!(delta <= 8, "步进 5‱ 的颜色跳变过大: {delta} (bps={bps})");
                }
            }
            prev = next;
        }
    }

    /// edge_radius = 0 退化为硬边:播放头列并入已播渐变,其后立即轨道色,
    /// 全程只有纯色、无混色列(即「纯渐变生长」形态)。
    #[test]
    fn edge_radius_zero_is_hard_edge() {
        let theme = Theme::default();
        let played = Color::Rgb(200, 0, 0);
        let points = vec![255u8; 200];
        let bar_w = 40usize;
        let filled = 20usize;
        let cells = cells(&waveform_spans(
            &points,
            bar_w,
            Bps::new(5_000), // 5000‱ × 40 = 已播 20 列
            Bps::FULL,
            &PlayedStyle::Solid(played),
            /*contrast*/ 1.0,
            /*edge_radius*/ 0,
            &theme,
        ));
        for (col, (_, c)) in cells.iter().enumerate() {
            let expected = if col <= filled { played } else { theme.overlay };
            assert_eq!(*c, expected, "硬边下第 {col} 列必须是纯色");
        }
    }
}
