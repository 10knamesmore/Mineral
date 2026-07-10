//! 进度条振幅波形:包络重采样到列宽 → 块字符字形 → 三段着色 span 序列。
//!
//! 只做纯渲染零件;开关判定与包络归属校验在调用方(transport)。着色沿用普通
//! 进度条的三级层次语义:已播(亮)> 已缓冲(中灰)> 未缓冲(暗灰),播放头单列高亮。

use mineral_audio::Bps;
use mineral_model::Envelope;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use crate::components::layout::shared::transport::split_buffered_track;
use crate::render::theme::Theme;
use crate::runtime::state::AppState;

/// 波形渲染上下文(逐帧从应用状态现读构造,不缓存配置)。
pub struct WaveformCtx<'a> {
    /// 总开关(`tui.waveform.enabled` 现读,overlay 翻转即热生效)。
    pub enabled: bool,

    /// 已播放段颜色(封面主色或主题 accent_2,见 [`Self::new`])。
    pub played: Color,

    /// 当前曲包络(归属已校验);`None` = 未就绪,回落普通进度条。
    pub envelope: Option<&'a Envelope>,
}

impl<'a> WaveformCtx<'a> {
    /// 从应用状态现读构造:包络经 [`crate::runtime::playback::Playback::current_envelope`]
    /// 归属校验;已播色在 `cover_color` 开且当前曲封面 palette 就绪时取其亮位采样
    /// (与频谱同一份 kmeans 产物),否则回落主题 accent_2(与普通进度条已播段同色,
    /// palette 迟到时前后帧只是颜色渐变,无布局跳动)。
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
                .playback
                .track
                .as_ref()
                .and_then(|t| t.cover_url.as_ref())
                .and_then(|url| state.covers.palettes.get(url))
                .map_or(theme.accent_2, |palette| {
                    palette.sample(/*pos_permille*/ 700)
                })
        } else {
            theme.accent_2
        };
        Self {
            enabled: *cfg.enabled(),
            played,
            envelope: state.playback.current_envelope(),
        }
    }

    /// 关闭态上下文(测试用):恒回落普通进度条。
    #[cfg(test)]
    pub fn off() -> WaveformCtx<'static> {
        WaveformCtx {
            enabled: false,
            played: Color::Reset,
            envelope: None,
        }
    }
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

/// 波形进度条 span 序列(总 cell 宽恒 == `bar_w`,布局不抖)。
///
/// # Params:
///   - `points`: 包络点(0..=255)
///   - `bar_w`: 目标列数
///   - `filled`: 已播列数(内部 clamp 到 `bar_w`)
///   - `buffered`: 已缓冲比例(播放头之后拆亮 / 暗两段,与普通进度条同语义)
///   - `played`: 已播段颜色
///   - `theme`: 取色主题
///
/// # Return:
///   按颜色分段合并后的 span 序列。
pub(crate) fn waveform_spans(
    points: &[u8],
    bar_w: usize,
    filled: usize,
    buffered: Bps,
    played: Color,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let glyphs = resample_columns(points, bar_w)
        .iter()
        .map(|&h| glyph_for(h))
        .collect::<Vec<&'static str>>();
    let filled = filled.min(bar_w);
    let run = |range: std::ops::Range<usize>| -> String {
        glyphs.get(range).unwrap_or_default().concat()
    };
    let mut spans = Vec::new();
    if filled > 0 {
        spans.push(Span::styled(run(0..filled), Style::new().fg(played)));
    }
    if filled < bar_w {
        // 播放头:单列亮色加粗(字形仍是该列振幅块,不换字符故布局不抖)。
        spans.push(Span::styled(
            run(filled..filled + 1),
            Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
        ));
        let (bright, dim) = split_buffered_track(bar_w, filled, buffered);
        let bright_end = filled + 1 + bright;
        if bright > 0 {
            spans.push(Span::styled(
                run(filled + 1..bright_end),
                Style::new().fg(theme.overlay),
            ));
        }
        if dim > 0 {
            spans.push(Span::styled(
                run(bright_end..bright_end + dim),
                Style::new().fg(theme.surface0),
            ));
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use mineral_audio::Bps;
    use ratatui::style::Color;
    use ratatui::text::Span;

    use super::{glyph_for, resample_columns, waveform_spans};
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

    /// span 序列 cell 宽守恒 == bar_w;已播段用 played 色、播放头单列高亮、
    /// 其后按缓冲拆亮暗两段(与普通进度条同语义)。
    #[test]
    fn spans_conserve_width_and_partition_colors() {
        let theme = Theme::default();
        let played = Color::Rgb(226, 184, 107);
        let points = vec![128u8; 200];
        let bar_w = 40usize;
        let filled = 10usize;
        let spans = waveform_spans(&points, bar_w, filled, Bps::new(5_000), played, &theme);
        let cells = cells(&spans);
        assert_eq!(cells.len(), bar_w, "波形总宽必须恰等于 bar_w,布局不抖");

        let played_cells = cells.iter().take(filled);
        assert!(
            played_cells.clone().count() == filled
                && played_cells.clone().all(|(_, c)| *c == played),
            "前 filled 列应整体用已播色"
        );
        // 播放头单列:亮色(theme.text)。
        assert_eq!(cells.get(filled).map(|(_, c)| *c), Some(theme.text));
        // 其后:先 overlay(已缓冲)后 surface0(未缓冲),两色不交错。
        let track = cells
            .iter()
            .skip(filled + 1)
            .collect::<Vec<&(char, Color)>>();
        let last_bright = track.iter().rposition(|(_, c)| *c == theme.overlay);
        let first_dim = track.iter().position(|(_, c)| *c == theme.surface0);
        if let (Some(lb), Some(fd)) = (last_bright, first_dim) {
            assert!(lb < fd, "已缓冲亮段应全部在未缓冲暗段之前");
        }
        assert!(
            track
                .iter()
                .all(|(_, c)| *c == theme.overlay || *c == theme.surface0),
            "播放头之后只该有 overlay/surface0 两色"
        );
    }
}
