//! 将当前歌曲的响度包络映射为单行块字符，并逐列揭示；配色由进度条统一处理。

use crate::config::WaveformConfig;

use crate::render::anim::ease_out;
use crate::render::theme::permille_of;
use crate::runtime::playback::Playback;

/// 当前帧的一列波形；未揭示时保留普通进度条字形。
pub(super) struct WaveformColumn {
    /// 已揭示的块字符；None 表示仍显示进度线。
    pub glyph: Option<&'static str>,

    /// 入场前沿向主题 text 提亮的强度，千分比；揭示完成后归零。
    pub glow: u16,
}

/// 按当前列宽重采样包络并计算入场状态；关闭波形或包络未就绪时返回 None。
pub(super) fn columns(
    playback: &Playback,
    cfg: &WaveformConfig,
    width: usize,
) -> Option<Vec<WaveformColumn>> {
    let envelope = playback.current_envelope().filter(|_| *cfg.enabled())?;
    let reveal = RevealStyle {
        sweep: permille_of(*cfg.reveal().sweep_ratio()),
        glow: permille_of(*cfg.reveal().glow()),
    };
    Some(
        resample_columns(&envelope.envelope().points, width)
            .into_iter()
            .enumerate()
            .map(|(col, height)| {
                let progress = reveal.column(col, width, envelope.reveal());
                let height = u8::try_from(
                    u32::from(apply_contrast(height, *cfg.contrast())) * u32::from(progress)
                        / u32::from(FULL_E3),
                )
                .unwrap_or(u8::MAX);
                WaveformColumn {
                    glyph: (progress > 0).then(|| glyph_for(height)),
                    glow: u16::try_from(
                        u32::from(FULL_E3 - progress) * u32::from(reveal.glow) / u32::from(FULL_E3),
                    )
                    .unwrap_or(FULL_E3),
                }
            })
            .collect(),
    )
}

/// 波形入场的横扫与提亮参数，每帧从配置读取。
struct RevealStyle {
    /// 横扫占总时长的比例，千分比；余下时长供每列纵向生长。
    sweep: u16,

    /// 正在揭示的列向主题 text 提亮的强度，千分比。
    glow: u16,
}

impl RevealStyle {
    /// 按列位置错开生长起点，返回当前列经过缓动的揭示进度，千分比。
    fn column(&self, col: usize, bar_w: usize, progress: u16) -> u16 {
        let sweep = self.sweep.min(FULL_E3);
        // 末列在 sweep 处起跑，生长结束恰好落在动画总时长处。
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
            return FULL_E3;
        }
        ease_out(
            u16::try_from(u32::from(elapsed) * u32::from(FULL_E3) / u32::from(grow))
                .unwrap_or(FULL_E3)
                .min(FULL_E3),
        )
    }
}

/// 千分比满值，与入场动画进度同量纲。
const FULL_E3: u16 = 1000;

/// 对归一响度施加 gamma，保留 0 和 255 两端；非正指数或 NaN 按线性映射。
#[allow(clippy::as_conversions)] // reason: 浮点幂映射到 u8,值域已 clamp 进 0..=255
fn apply_contrast(height: u8, contrast: f32) -> u8 {
    if !(contrast.is_finite() && contrast > 0.0) || (contrast - 1.0).abs() < f32::EPSILON {
        return height;
    }
    let normalized = f32::from(height) / 255.0;
    (normalized.powf(contrast) * 255.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// 单个终端格内的八级振幅高度。
const LADDER: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// 将振幅 0-255 量化为一个字符格；零振幅仍保留最低轨道线。
fn glyph_for(height: u8) -> &'static str {
    let index = usize::from(height) * LADDER.len() / 256;
    LADDER.get(index).copied().unwrap_or("█")
}

/// 缩小时按桶取峰以保留瞬态，放大时线性插值；输入或列宽为空时返回空列。
fn resample_columns(points: &[u8], columns: usize) -> Vec<u8> {
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
        // 输入为 u8，256 分度定点插值足以保留原始精度。
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

#[cfg(test)]
mod tests {
    use super::resample_columns;

    /// 包络缩小与放大都产生请求的列数；空输入不产生波形。
    #[test]
    fn resample_matches_column_count() {
        assert_eq!(resample_columns(&[128u8; 200], 10).len(), 10);
        assert_eq!(resample_columns(&[0, 128, 255], 10).len(), 10);
        assert_eq!(resample_columns(&[], 10), Vec::<u8>::new());
        assert_eq!(resample_columns(&[128u8; 200], 0), Vec::<u8>::new());
    }

    /// 缩小按桶取峰，单点瞬态不会被平均抹掉。
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
            "突刺列应保留原始峰值:{columns:?}"
        );
    }
}
