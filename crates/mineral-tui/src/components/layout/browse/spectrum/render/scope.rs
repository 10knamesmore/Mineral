//! 示波器画法:时域 min/max 包络右新左旧匀速滚动,每 dot 列画一段跨中线的
//! 竖直 Braille 点段。
//!
//! 列在 state 侧按 `scope.column_ms` 与**音频时间**对齐,渲染只做「尾部贴右缘」
//! 的搬运,不重采样——滚动速度恒定,不随样本到达节奏抖动。历史不足面板宽时
//! 左侧退化成中线(从未有数据 = 整条中线,面板不死寂);暂停时整幅冻结可观察。

use ratatui::Frame;
use ratatui::layout::Rect;

use super::super::state::SpectrumState;
use super::BrailleGrid;
use crate::components::layout::shared::text::center_bg;
use crate::render::color::{ensure_bg_contrast, lerp_color, luma255_of};
use crate::render::palette::ColumnColors;
use crate::render::theme::Theme;

/// 渲染示波器到面板内区。
#[allow(clippy::as_conversions)] // 浮点几何 → 点坐标:量级 ≤ 数千,f32 内精确
pub(super) fn paint(frame: &mut Frame<'_>, area: Rect, state: &SpectrumState, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let cols = usize::from(area.width);
    state.target_bars.set(cols.max(1));
    let point_w = cols * 2;
    let point_h = usize::from(area.height) * 4;
    let center = point_h as f32 / 2.0;
    let amplitude = (center - 2.0).max(1.0);
    // 端点色每字符列算一次:深包络下逐点算是每帧几千次端点计算(封面过渡期尤重),
    // 同列端点本就相同,hoist 无视觉差。端点过亮度保底:氛围背景与谱色同源(同一
    // 封面调色板),撞色时细小盲文点会融进场里,按实际背景抬开最小亮度差。
    let bg = center_bg(frame, area);
    let floor = luma255_of(*state.cfg().dot_bg_contrast());
    let endpoints = (0..cols)
        .map(|col| {
            let column = state.column_colors(col, cols.max(1), theme);
            ColumnColors {
                bottom: ensure_bg_contrast(column.bottom, bg, floor),
                top: ensure_bg_contrast(column.top, bg, floor),
            }
        })
        .collect::<Vec<ColumnColors>>();
    let mut grid = BrailleGrid::new(cols, usize::from(area.height));
    for px in 0..point_w {
        // 最新列贴右缘,向左回溯历史;够不着的(历史短 / 从未有数据)画中线。
        let span = state.wave_span_from_newest(point_w - 1 - px);
        let (low, high) = span.map_or((0.0_f32, 0.0_f32), |s| {
            (s.min.clamp(-1.0, 1.0), s.max.clamp(-1.0, 1.0))
        });
        let y_top = (center - high * amplitude).round() as isize;
        let y_bottom = (center - low * amplitude).round() as isize;
        let Some(column) = endpoints.get(px / 2) else {
            continue;
        };
        for py in y_top.min(y_bottom)..=y_top.max(y_bottom) {
            if py < 0 {
                continue;
            }
            let py = usize::try_from(py).unwrap_or(usize::MAX);
            if py >= point_h {
                continue;
            }
            // 离中线越远越亮:中线贴列底色、峰顶贴列顶色(与条形的垂直渐变同构)。
            let deviation = ((py as f32 - center).abs() / amplitude).clamp(0.0, 1.0);
            let permille = (deviation * 1000.0) as u64;
            let color = lerp_color(column.bottom, column.top, permille, 1000);
            grid.set(px, py, color);
        }
    }
    grid.blit(frame.buffer_mut(), area);
}
