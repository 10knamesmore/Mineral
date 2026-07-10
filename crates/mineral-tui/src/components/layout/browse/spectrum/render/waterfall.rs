//! 频谱历史瀑布画法(热力半块):x=频率、y=时间,最新一帧在顶部、历史向下流走。
//!
//! `▀` 半块 fg=上帧 / bg=下帧,一字符行装两帧历史,幅度→颜色连续 lerp——终端一格
//! 只有前景/背景两色,这已是时间密度上限。热力色沿用逐列端点(hue 漂移 / 封面色场
//! 自动通吃):下半程从面板衬底色升到列底色、上半程升到列顶色;幅度 0 不落笔,
//! 保持面板原底。
//!
//! 历史行按推行那刻的列数存,这里按当前面板宽线性插值读取(与 terrain 同策略)
//! ——browse↔fullscreen 切换 / resize 面板宽变化时旧历史照常显示,不清空重攒。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use super::super::state::{SPECTRUM_RES, SpectrumState};
use crate::render::color::lerp_color;
use crate::render::palette::ColumnColors;
use crate::render::theme::Theme;

/// 渲染瀑布到面板内区。
pub(super) fn paint(frame: &mut Frame<'_>, area: Rect, state: &SpectrumState, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let cols = usize::from(area.width).max(1);
    state.target_bars.set(cols);
    // 对比 gamma 现读,整面共用;幅度→色前统一按它重标(见 [`contrast_gamma`])。
    let contrast = *state.cfg().waterfall().contrast();
    // 端点色每字符列算一次,整面历史共用(热力色只差幅度档)。
    let endpoints = (0..cols)
        .map(|col| state.column_colors(col, cols, theme))
        .collect::<Vec<ColumnColors>>();
    let buf = frame.buffer_mut();
    for y in 0..area.height {
        let up = state.water_row(usize::from(y) * 2);
        let down = state.water_row(usize::from(y) * 2 + 1);
        if up.is_none() && down.is_none() {
            break; // 历史尽头,更下方全空
        }
        for (x, column) in endpoints.iter().enumerate() {
            let v_up = up.map_or(0, |row| sample_row(row, x, cols));
            let v_down = down.map_or(0, |row| sample_row(row, x, cols));
            let cx = area.x + u16::try_from(x).unwrap_or(0);
            let cy = area.y + y;
            // 三态:双帧有声画双色 ▀;单侧有声画对应半块,另一半留面板原底。
            let (glyph, style) = match (v_up > 0, v_down > 0) {
                (true, true) => (
                    "▀",
                    Style::new()
                        .fg(heat_color(
                            v_up,
                            contrast,
                            column.bottom,
                            column.top,
                            theme.surface0,
                        ))
                        .bg(heat_color(
                            v_down,
                            contrast,
                            column.bottom,
                            column.top,
                            theme.surface0,
                        )),
                ),
                (true, false) => (
                    "▀",
                    Style::new().fg(heat_color(
                        v_up,
                        contrast,
                        column.bottom,
                        column.top,
                        theme.surface0,
                    )),
                ),
                (false, true) => (
                    "▄",
                    Style::new().fg(heat_color(
                        v_down,
                        contrast,
                        column.bottom,
                        column.top,
                        theme.surface0,
                    )),
                ),
                (false, false) => continue,
            };
            buf.set_string(cx, cy, glyph, style);
        }
    }
}

/// 历史行在第 `x` 列(共 `cols` 列)的幅度:行数据按长度线性插值到当前面板列,
/// 行长与面板宽解耦。行长 = `cols` 时退化为精确索引(零插值误差)。
#[allow(clippy::as_conversions)] // 插值系数量级 ≤ RES(64),f32 内精确
fn sample_row(row: &[u16], x: usize, cols: usize) -> u16 {
    let Some(last_idx) = row.len().checked_sub(1) else {
        return 0;
    };
    let t = x as f32 / (cols.saturating_sub(1).max(1) as f32);
    let position = t * last_idx as f32;
    let i0 = (position.floor() as usize).min(last_idx);
    let fraction = position - i0 as f32;
    let a = f32::from(row.get(i0).copied().unwrap_or(0));
    let b = f32::from(row.get((i0 + 1).min(last_idx)).copied().unwrap_or(0));
    u16::try_from((a * (1.0 - fraction) + b * fraction).round() as i64).unwrap_or(0)
}

/// 幅度(0..=RES)→ 热力色:先按对比 gamma 重标幅度(见 [`contrast_gamma`]),再分两段——
/// 下半程 `surface0 → lo`(能量微弱时贴近衬底),上半程 `lo → hi`(响亮处顶到列顶色)。
fn heat_color(v: u16, contrast: f32, lo: Color, hi: Color, surface: Color) -> Color {
    let half = u64::from(SPECTRUM_RES / 2);
    let v = u64::from(contrast_gamma(v, contrast).min(SPECTRUM_RES));
    if v <= half {
        lerp_color(surface, lo, v, half)
    } else {
        lerp_color(lo, hi, v - half, half)
    }
}

/// 对比 gamma:`(v/RES)^contrast × RES`,端点(0 / RES)不动、单调。把中低能量段拉开——
/// `> 1` 压暗噪底、只留强峰(音高线更突出);`< 1` 抬亮弱能量(泛音/弱谐波浮现,噪声也起)。
/// `contrast == 1` 或非有限 / 非正值按线性(恒等)处理,避免 `powf` 在 gamma=1 时的精度抖动。
#[allow(clippy::as_conversions)] // reason: 浮点幂映射回 0..=RES,已 clamp
fn contrast_gamma(v: u16, contrast: f32) -> u16 {
    if !(contrast.is_finite() && contrast > 0.0) || (contrast - 1.0).abs() < f32::EPSILON {
        return v;
    }
    let res = f32::from(SPECTRUM_RES);
    let normalized = f32::from(v.min(SPECTRUM_RES)) / res;
    (normalized.powf(contrast) * res).round().clamp(0.0, res) as u16
}

#[cfg(test)]
mod tests {
    use super::{SPECTRUM_RES, contrast_gamma};

    /// contrast = 1 是恒等映射(显式关掉 gamma 时精确回到线性两段映射)。
    #[test]
    fn contrast_one_is_identity() {
        for v in 0..=SPECTRUM_RES {
            assert_eq!(contrast_gamma(v, 1.0), v, "gamma 1 必须恒等: v={v}");
        }
    }

    /// contrast > 1 端点不动、中段下沉、单调不升(压暗噪底、只留强峰)。
    /// RES=64、gamma=2:中点 32 → (0.5)² × 64 = 16。
    #[test]
    fn contrast_gt_one_darkens_midtones_and_keeps_order() {
        assert_eq!(contrast_gamma(0, 2.0), 0);
        assert_eq!(contrast_gamma(SPECTRUM_RES, 2.0), SPECTRUM_RES);
        assert_eq!(contrast_gamma(32, 2.0), 16);
        let mut prev = 0;
        for v in 0..=SPECTRUM_RES {
            let mapped = contrast_gamma(v, 2.0);
            assert!(mapped >= prev, "gamma 映射必须单调: v={v}");
            assert!(mapped <= v, "gamma > 1 不得抬升任何点: v={v}");
            prev = mapped;
        }
    }

    /// contrast < 1 抬亮弱能量(泛音/弱谐波浮现):端点不动,中段上浮。
    /// gamma=0.5:中点 32 → √0.5 × 64 ≈ 45。
    #[test]
    fn contrast_lt_one_brightens_midtones() {
        assert_eq!(contrast_gamma(0, 0.5), 0);
        assert_eq!(contrast_gamma(SPECTRUM_RES, 0.5), SPECTRUM_RES);
        assert_eq!(contrast_gamma(32, 0.5), 45);
        for v in 1..SPECTRUM_RES {
            assert!(
                contrast_gamma(v, 0.5) >= v,
                "gamma < 1 不得压低任何点: v={v}"
            );
        }
    }

    /// 非法 contrast(非有限 / 非正)退化为线性恒等,不 panic。
    #[test]
    fn contrast_invalid_falls_back_to_identity() {
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(contrast_gamma(20, bad), 20, "非法 gamma={bad} 应恒等");
        }
    }
}
