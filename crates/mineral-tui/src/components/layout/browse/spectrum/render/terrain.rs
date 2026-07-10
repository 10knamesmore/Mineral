//! 山脊地形画法:最前山脊 = 当前 ADSR 平滑条(固定在底部的「现在」),历史层
//! 按推层进度连续上浮,越旧越高越沉,前景遮挡后景形成纵深(Braille 亚格分辨率
//! 画连续轮廓线)。
//!
//! 平滑滚动:历史层 k 画在 `base − (k + progress) × 层距`,progress ∈ [0,1) 每拍
//! 推进;推层瞬间 progress 归零、层序整体 +1,新历史层恰好从最前山脊的位置接棒,
//! 位置与亮度都连续——山脊匀速向上漂,而非每 `push_ms` 整层跳一格。亮度随深度
//! 衰减到 `terrain.fade_floor` 保底(远层仍可辨、不隐入衬底),最旧层浮出顶部时
//! 平滑退场。
//!
//! 遮挡是画家算法:最前山脊先画并立「地平线」(每 dot 列已画的最高点),更旧的
//! 层只在地平线之上可见;层内可见性按**本层动笔前**的地平线判定,画完统一压低
//! ——边画边更会让陡坡的竖直连线被自己遮挡。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::super::state::{SPECTRUM_RES, SpectrumState};
use super::BrailleGrid;
use crate::render::color::lerp_color;
use crate::render::palette::ColumnColors;
use crate::render::theme::Theme;

/// 渲染地形到面板内区。
#[allow(clippy::as_conversions)] // 浮点几何 → 点坐标:量级 ≤ 数千,f32 内精确
pub(super) fn paint(frame: &mut Frame<'_>, area: Rect, state: &SpectrumState, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let cols = usize::from(area.width);
    let rows = usize::from(area.height);
    state.target_bars.set(cols.max(1));
    let point_w = cols * 2;
    let point_h = rows * 4;
    let layers = (*state.cfg().terrain().layers()).max(1);
    let base_y = point_h as f32 - 1.0;
    let mut painter = RidgePainter {
        grid: BrailleGrid::new(cols, rows),
        horizon: vec![point_h as isize; point_w],
        // 端点色每字符列算一次,全部层共用(层色只差亮度系数)。
        endpoints: (0..cols)
            .map(|col| state.column_colors(col, cols.max(1), theme))
            .collect::<Vec<ColumnColors>>(),
        dim_anchor: theme.surface1,
        point_w,
        point_h,
        amplitude: point_h as f32 * (*state.cfg().terrain().amplitude()).clamp(0.0, 1.0),
        layers_f: layers as f32,
        fade_floor: (*state.cfg().terrain().fade_floor()).clamp(0.0, 1.0),
    };
    // 最前山脊:当前平滑条,深度 0(最亮),固定在 base。推层那刻的快照即历史层 0,
    // 内容与它一致 → 新层从这里无缝起浮。
    painter.draw_ridge(state.smoothed_bars(), base_y, /*depth*/ 0.0);
    let lift = (point_h as f32 - 6.0).max(1.0) / layers as f32;
    let progress = state.terrain_progress();
    for k in 0..layers {
        let Some(layer) = state.terrain_layer(k) else {
            break;
        };
        let depth = k as f32 + progress;
        painter.draw_ridge(layer, base_y - depth * lift, depth);
    }
    painter.grid.blit(frame.buffer_mut(), area);
}

/// 一次 paint 内跨层共享的画布 + 几何 / 配色环境(拆 [`Self::draw_ridge`] 只为
/// 收参数,不跨帧存活)。
struct RidgePainter {
    /// Braille 画布(首写优先着色,配「前景先画」= 遮挡自然成立)。
    grid: BrailleGrid,

    /// 每 dot 列的地平线:已画内容的最高点(y 越小越高),初始在画布外(最低)。
    horizon: Vec<isize>,

    /// 每字符列的底/顶端点色(paint 入口 hoist 一次)。
    endpoints: Vec<ColumnColors>,

    /// 山脊色亮度衰减的暗端锚点(锚在比面板衬底略亮的 `surface1`,让远层轮廓不隐入背景)。
    dim_anchor: Color,

    /// 画布点宽。
    point_w: usize,

    /// 画布点高。
    point_h: usize,

    /// 轮廓振幅(点),= 点高 × `terrain.amplitude`。须明显小于层距可用纵深,
    /// 否则层间交叠过深、山脊互相淹没不可辨。
    amplitude: f32,

    /// 总层数(f32,亮度衰减分母)。
    layers_f: f32,

    /// 远层亮度保底(0..=1):越旧的层亮度衰减到此为止,不再往 `dim_anchor` 以下沉。
    /// 0 = 远层淡到全隐(旧行为);1 = 全层等亮、无纵深。
    fade_floor: f32,
}

impl RidgePainter {
    /// 画一条山脊轮廓:`data` 插值到 dot 列,基线 `base_y`,亮度随 `depth` 衰减。
    /// 只在当前地平线之上落笔,画完把地平线压到本层轮廓。
    #[allow(clippy::as_conversions)] // 浮点几何 → 点坐标:量级 ≤ 数千,f32 内精确
    fn draw_ridge(&mut self, data: &[f32], base_y: f32, depth: f32) {
        let brightness = depth_brightness(depth, self.layers_f, self.fade_floor);
        let mut prev_y: Option<isize> = None;
        for px in 0..self.point_w {
            let v = sample(data, px, self.point_w);
            let y = (base_y - v * self.amplitude).round() as isize;
            // 陡坡补竖直连线,否则轮廓在斜率 > 1 处断成散点。
            let from = prev_y.map_or(y, |p| {
                if p < y {
                    (p + 1).min(y)
                } else {
                    (p - 1).max(y)
                }
            });
            let horizon_before = self.horizon.get(px).copied().unwrap_or(0);
            let color = self.ridge_color(px, brightness, v);
            for py in from.min(y)..=from.max(y) {
                if py >= 0 && py < horizon_before && py < self.point_h as isize {
                    self.grid
                        .set(px, usize::try_from(py).unwrap_or(usize::MAX), color);
                }
            }
            if let Some(slot) = self.horizon.get_mut(px) {
                *slot = (*slot).min(from.min(y));
            }
            prev_y = Some(y);
        }
    }

    /// 山脊在 `px` 处的落笔色:近层 + 高处亮,衬底色 → 列顶端点色。
    #[allow(clippy::as_conversions)] // 千分比定点换算,量级 ≤ 1000
    fn ridge_color(&self, px: usize, brightness: f32, v: f32) -> Color {
        let top = self
            .endpoints
            .get((px / 2).min(self.endpoints.len().saturating_sub(1)))
            .map_or(self.dim_anchor, |column| column.top);
        let permille = ((brightness * (0.35 + 0.65 * v)).clamp(0.0, 1.0) * 1000.0) as u64;
        lerp_color(self.dim_anchor, top, permille, 1000)
    }
}

/// 轮廓在 `px` 处的归一化高度(0..=1):数据按长度线性插值到点列,
/// 层长与面板宽解耦(resize 间隙的旧层照常显示,无需清历史)。
#[allow(clippy::as_conversions)] // 同上:插值系数量级小,f32 内精确
fn sample(layer: &[f32], px: usize, point_w: usize) -> f32 {
    let Some(last_idx) = layer.len().checked_sub(1) else {
        return 0.0;
    };
    let t = px as f32 / (point_w.saturating_sub(1).max(1) as f32);
    let position = t * last_idx as f32;
    let i0 = (position.floor() as usize).min(last_idx);
    let fraction = position - i0 as f32;
    let a = layer.get(i0).copied().unwrap_or(0.0);
    let b = layer.get((i0 + 1).min(last_idx)).copied().unwrap_or(a);
    ((a * (1.0 - fraction) + b * fraction) / f32::from(SPECTRUM_RES)).clamp(0.0, 1.0)
}

/// 深度 → 山脊亮度系数:近层(`depth` 0)满亮 1.0,越旧越暗,但**保底不低于**
/// `fade_floor`——远层不再往 `dim_anchor` 以下沉、隐入背景。`fade_floor = 0` 复原
/// 「淡到全隐」的旧行为,`= 1` 则全层等亮(无纵深)。
///
/// # Params:
///   - `depth`: 层深度(0 = 最前山脊,越旧越大;含推层进度小数)
///   - `layers`: 总层数(衰减分母,至少按 1 计)
///   - `fade_floor`: 远层亮度保底(clamp 到 0..=1)
///
/// # Return:
///   亮度系数,落在 `fade_floor ..= 1.0`。
fn depth_brightness(depth: f32, layers: f32, fade_floor: f32) -> f32 {
    let floor = fade_floor.clamp(0.0, 1.0);
    let raw = (1.0 - depth / layers.max(1.0)).clamp(0.0, 1.0);
    floor + (1.0 - floor) * raw
}

#[cfg(test)]
mod tests {
    use super::depth_brightness;

    /// 最前层(depth 0)恒满亮,与保底无关。
    #[test]
    fn front_layer_full_brightness() {
        for floor in [0.0, 0.3, 1.0] {
            assert!((depth_brightness(0.0, 8.0, floor) - 1.0).abs() < f32::EPSILON);
        }
    }

    /// 最旧层(depth ≥ layers)落到保底 `fade_floor`,不再更暗;`= 0` 复原旧行为(全隐)。
    #[test]
    fn oldest_layer_clamped_to_floor() {
        assert!((depth_brightness(8.0, 8.0, 0.30) - 0.30).abs() < 1e-6);
        assert!((depth_brightness(20.0, 8.0, 0.30) - 0.30).abs() < 1e-6);
        assert!(depth_brightness(8.0, 8.0, 0.0).abs() < f32::EPSILON);
    }

    /// 亮度随深度单调不升,且恒 ≥ 保底。
    #[test]
    fn monotonic_and_bounded_by_floor() {
        let floor = 0.30;
        let mut prev = f32::INFINITY;
        for k in 0..=8_u8 {
            let b = depth_brightness(f32::from(k), 8.0, floor);
            assert!(b <= prev + 1e-6, "亮度应单调不升: depth={k}");
            assert!(b >= floor - 1e-6, "亮度不得低于保底: depth={k}");
            prev = b;
        }
    }

    /// `fade_floor = 1` 时全层等亮(无纵深)。
    #[test]
    fn full_floor_is_flat() {
        for depth in [0.0, 3.0, 8.0] {
            assert!((depth_brightness(depth, 8.0, 1.0) - 1.0).abs() < f32::EPSILON);
        }
    }
}
