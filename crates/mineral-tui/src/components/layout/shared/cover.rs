//! 程序化半字符 (`▀`) 封面 — 名称 FNV-1a hash → 选算法 + 8 索引调色盘。
//!
//! 每个字符存储 2 个逻辑像素(上 / 下),配合 `▀` 的 fg/bg 实现宽:高 = 1:2
//! 的方形像素;cover 整体保持 ~2:1 字符比,方形视觉。

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use crate::render::theme::Theme;

/// 在 `area` 左上方画一个程序化封面(seed = 歌单名 / 专辑名)。
///
/// `area` 宽高决定封面字符尺寸,完全响应式:`cell_w = min(width, height*2)`,
/// `cell_h = cell_w / 2`。
pub fn render(frame: &mut Frame<'_>, area: Rect, seed: &str, theme: &Theme) {
    render_to(frame.buffer_mut(), area, seed, theme);
}

/// [`render`] 的 [`Buffer`] 版:离屏合成(detail 下钻 sweep)直接画进给定缓冲。
///
/// # Params:
///   - `buf`: 目标缓冲(屏幕或离屏)
///   - `area`: 封面区域
///   - `seed`: 程序化封面种子(歌单名 / 专辑名)
pub fn render_to(buf: &mut Buffer, area: Rect, seed: &str, theme: &Theme) {
    let sq = square_cells(area);
    if sq.width == 0 || sq.height == 0 {
        return;
    }
    let palette: [Color; 8] = [
        theme.crust,
        theme.surface0,
        theme.overlay,
        theme.subtext,
        theme.text,
        theme.accent,
        theme.accent_2,
        theme.peach,
    ];
    let h = hash(seed);
    for cy in 0..sq.height {
        for cx in 0..sq.width {
            let top = pixel(h, cx, cy.saturating_mul(2), sq.width);
            let bot = pixel(h, cx, cy.saturating_mul(2).saturating_add(1), sq.width);
            let fg = palette
                .get(usize::from(top) & 7)
                .copied()
                .unwrap_or(theme.text);
            let bg = palette
                .get(usize::from(bot) & 7)
                .copied()
                .unwrap_or(theme.base);
            let style = Style::new().fg(fg).bg(bg);
            buf.set_string(sq.x + cx, sq.y + cy, "▀", style);
        }
    }
}

/// 程序化 / halfblock 封面共用的「正方 cell 区」:横向取 `min(width, 2*height)` 居中,
/// 高度减半(`▀` 半字符宽:高 = 1:2,cell 高 = cell 宽 / 2 即得方形视觉)。
///
/// 与 `cover_image` 的 `square_subarea`(按真实字号比算)是两套正方化:程序化封面 / 离屏
/// halfblock 占位用这套(无需字号、确定性);屏上真图用字号那套(与稳态 kitty 落点严丝合缝)。
/// 二者差异仅在字号偏离 1:2 时显现,且同一封面一帧内非图即程序化、不会半途换算法。
///
/// # Params:
///   - `area`: 外部可用区
///
/// # Return:
///   居中正方 cell 区;`area` 宽高任一为 0 时返回原点处零面积 rect。
pub(crate) fn square_cells(area: Rect) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let cell_w = area.width.min(area.height.saturating_mul(2));
    let cell_h = cell_w / 2;
    let off_x = area.x + area.width.saturating_sub(cell_w) / 2;
    Rect::new(off_x, area.y, cell_w, cell_h)
}

/// 在 `(x,y)` 处采样一个 8 索引调色盘的像素;按 hash 的高 2 位选 horizon/mondrian/concentric/stripes。
fn pixel(h: u32, x: u16, y: u16, w: u16) -> u8 {
    let cx = u32::from(x);
    let cy = u32::from(y);
    let cw = u32::from(w).max(1);
    let variant = (h >> 24) & 0x03;
    let v = match variant {
        0 => horizon(cy, cw, h),
        1 => mondrian(cx, cy, cw, h),
        2 => concentric(cx, cy, cw, h),
        _ => stripes(cx, cy, h),
    };
    u8::try_from(v & 0xff).unwrap_or(0)
}

/// 「地平线」算法:上半天空 (accent/accent_2 间隔)、下半地面 (surface0/overlay 间隔)。
fn horizon(cy: u32, cw: u32, h: u32) -> u32 {
    let mid = cw / 2;
    if cy < mid {
        // 天空:5(accent) / 6(accent_2) 间隔
        5 + ((cy ^ h) & 1)
    } else {
        // 地面:1(surface0) / 2(overlay)
        1 + ((cy ^ h) & 1)
    }
}

/// 「蒙德里安」算法:把图分成 1/4 边长的方块,每块按 hash 取 0..8 的色号。
fn mondrian(cx: u32, cy: u32, cw: u32, h: u32) -> u32 {
    let zw = (cw / 4).max(1);
    let zx = cx / zw;
    let zy = cy / zw;
    let key = zx ^ zy.wrapping_mul(7) ^ h;
    key & 7
}

/// 「同心圆」算法:按到中心的 r² 分环,环号 ^ hash 决定色号。
fn concentric(cx: u32, cy: u32, cw: u32, h: u32) -> u32 {
    let half = i32::try_from(cw / 2).unwrap_or(0);
    let dx = i32::try_from(cx).unwrap_or(0) - half;
    let dy = i32::try_from(cy).unwrap_or(0) - half;
    let r2 = dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy));
    let band = u32::try_from(r2).unwrap_or(0) / (cw.max(1) + 1);
    band ^ (h >> 16)
}

/// 「斜条纹」算法:`(cx + cy) mod 8` ^ hash 高位。
fn stripes(cx: u32, cy: u32, h: u32) -> u32 {
    let v = (cx + cy) & 7;
    v ^ (h >> 8)
}

/// FNV-1a 32 位 hash,字符串 → 32 位 seed,各算法用它的不同位段做差异化。
fn hash(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(16_777_619);
    }
    h
}
