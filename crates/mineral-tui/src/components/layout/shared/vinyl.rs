//! not playing 待机封面：旋转唱片纹(`▀` 半字符逐 cell 绘制)。
//!
//! 同心沟纹 + 双对称高光瓣绕盘心缓慢旋转 + peach 标贴中央嵌 ◆ 徽记;颜色全取主题调色盘,
//! 高光衰减边缘用 4×4 Bayer 有序抖动落到「亮/不亮」两档。纯 cell 绘制、不碰终端图协议,
//! 逐帧重画安全,形变期与稳态走同一条路。

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use crate::render::theme::Theme;

/// 待机唱片纹的旋转状态:相位计数 + 一圈总步数(挂 `AppState`,主循环每 tick 推进一步)。
///
/// 一圈步数由配置 `animation.vinyl_rev_ms` 按主循环帧间隔折算,转一圈的真实时长与帧率解耦。
pub struct VinylSpin {
    /// 当前相位(`0..steps_per_rev` 回绕)。
    phase: u16,

    /// 一圈总步数(≥ 1)。
    steps_per_rev: u16,
}

impl VinylSpin {
    /// 从配置折算:一圈步数 = `rev_ms / tick_ms`,clamp 到 `1..=u16::MAX`。
    ///
    /// # Params:
    ///   - `rev_ms`: 旋转一圈的毫秒数(配置 `animation.vinyl_rev_ms`)
    ///   - `tick_ms`: 主循环帧间隔(配置 `animation.frame_tick_ms`)
    pub(crate) fn from_config(rev_ms: u32, tick_ms: u64) -> Self {
        let steps = (u64::from(rev_ms) / tick_ms.max(1)).clamp(1, u64::from(u16::MAX));
        Self {
            phase: 0,
            steps_per_rev: u16::try_from(steps).unwrap_or(1),
        }
    }

    /// 推进一步(主循环每 tick 恰调一次),整圈回绕。
    pub(crate) fn tick(&mut self) {
        self.phase = self.phase.wrapping_add(1) % self.steps_per_rev.max(1);
    }

    /// 当前旋转角(弧度)。
    fn angle(&self) -> f32 {
        std::f32::consts::TAU * f32::from(self.phase) / f32::from(self.steps_per_rev.max(1))
    }
}

/// 盘外背景的半径下界(归一化半径,下同)。
const DISC_R: f32 = 0.98;

/// 轴孔半径。
const HOLE_R: f32 = 0.06;

/// ◆ 徽记的曼哈顿半径(`|dx|+|dy|`,菱形)。
const BADGE_R: f32 = 0.16;

/// 标贴半径。
const LABEL_R: f32 = 0.24;

/// 标贴外描边半径。
const LABEL_EDGE_R: f32 = 0.27;

/// 沟纹环带频率(半径 → 明暗交替的带数标度)。
const GROOVE_FREQ: f32 = 46.0;

/// 高光瓣的角向宽度(弧度,高斯衰减标度)。
const SHEEN_WIDTH: f32 = 0.30;

/// 高光最亮的半径位置。
const SHEEN_MID_R: f32 = 0.60;

/// 高光沿半径偏离 [`SHEEN_MID_R`] 的衰减斜率。
const SHEEN_TAPER: f32 = 1.4;

/// 4×4 Bayer 有序抖动矩阵(0..16),终端上把连续高光强度落成两档的标准做法。
const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// 在 `area` 内画待机唱片纹，使用图片模块的 cell 正方几何。
pub fn render(frame: &mut Frame<'_>, area: Rect, spin: &VinylSpin, theme: &Theme) {
    render_to(frame.buffer_mut(), area, spin, theme);
}

/// [`render`] 的 [`Buffer`] 版:每个 cell 上/下两个逻辑像素分别采样,`▀` 的 fg/bg 落色。
pub fn render_to(buf: &mut Buffer, area: Rect, spin: &VinylSpin, theme: &Theme) {
    let sq = crate::image::square_cells(area);
    if sq.width == 0 || sq.height == 0 {
        return;
    }
    let px_h = sq.height.saturating_mul(2);
    let angle = spin.angle();
    for cy in 0..sq.height {
        for cx in 0..sq.width {
            let top = pixel(cx, cy.saturating_mul(2), sq.width, px_h, angle, theme);
            let bot = pixel(
                cx,
                cy.saturating_mul(2).saturating_add(1),
                sq.width,
                px_h,
                angle,
                theme,
            );
            let style = Style::new().fg(top).bg(bot);
            buf.set_string(sq.x + cx, sq.y + cy, "▀", style);
        }
    }
}

/// 采样一个逻辑像素:由内到外依次是轴孔 → ◆ 徽记 → 标贴 → 描边 → 沟纹盘面(叠高光瓣)→ 盘外。
fn pixel(x: u16, y: u16, w: u16, h: u16, angle: f32, theme: &Theme) -> Color {
    let dx = (f32::from(x) + 0.5) / f32::from(w.max(1)) * 2.0 - 1.0;
    let dy = (f32::from(y) + 0.5) / f32::from(h.max(1)) * 2.0 - 1.0;
    let r = (dx * dx + dy * dy).sqrt();
    if r > DISC_R {
        return theme.base;
    }
    if r < HOLE_R {
        return theme.crust;
    }
    if dx.abs() + dy.abs() < BADGE_R {
        return theme.accent;
    }
    if r < LABEL_R {
        return theme.peach;
    }
    if r < LABEL_EDGE_R {
        return theme.crust;
    }
    // 盘面:沟纹 = 半径环带明暗交替,上面叠双对称高光瓣(唱片反光是对置两条)。
    let groove = if (r * GROOVE_FREQ).rem_euclid(2.0) < 1.0 {
        theme.crust
    } else {
        theme.surface0
    };
    let delta = (dy.atan2(dx) - angle).rem_euclid(std::f32::consts::PI);
    let lobe = delta.min(std::f32::consts::PI - delta);
    let taper = (1.0 - (r - SHEEN_MID_R).abs() * SHEEN_TAPER).max(0.0);
    let sheen = (-(lobe / SHEEN_WIDTH) * (lobe / SHEEN_WIDTH)).exp() * taper;
    if sheen > bayer_threshold(x, y) {
        theme.overlay
    } else {
        groove
    }
}

/// `(x, y)` 处的 Bayer 抖动阈值(0..1)。
fn bayer_threshold(x: u16, y: u16) -> f32 {
    let v = BAYER
        .get(usize::from(y) % 4)
        .and_then(|row| row.get(usize::from(x) % 4))
        .copied()
        .unwrap_or(0);
    (f32::from(v) + 0.5) / 16.0
}
