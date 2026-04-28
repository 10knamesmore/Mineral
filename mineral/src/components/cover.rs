//! 程序化半字符 (`▀`) 封面 — 名称 FNV-1a hash → 选算法 + 8 索引调色盘。
//!
//! 每个字符存储 2 个逻辑像素(上 / 下),配合 `▀` 的 fg/bg 实现宽:高 = 1:2
//! 的方形像素;cover 整体保持 ~2:1 字符比,方形视觉。

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::Frame;

use crate::theme::Theme;

/// 在 `area` 左上方画一个程序化封面(seed = 歌单名 / 专辑名)。
///
/// `area` 宽高决定封面字符尺寸,完全响应式:`cell_w = min(width, height*2)`,
/// `cell_h = cell_w / 2`。
pub fn render(frame: &mut Frame<'_>, area: Rect, seed: &str, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let cell_w = area.width.min(area.height.saturating_mul(2));
    let cell_h = cell_w / 2;
    if cell_w == 0 || cell_h == 0 {
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
    // 居中放置封面到 area 内。
    let off_x = area.x + (area.width.saturating_sub(cell_w)) / 2;
    let off_y = area.y;
    let buf = frame.buffer_mut();
    for cy in 0..cell_h {
        for cx in 0..cell_w {
            let top = pixel(h, cx, cy.saturating_mul(2), cell_w);
            let bot = pixel(h, cx, cy.saturating_mul(2).saturating_add(1), cell_w);
            let fg = palette
                .get(usize::from(top) & 7)
                .copied()
                .unwrap_or(theme.text);
            let bg = palette
                .get(usize::from(bot) & 7)
                .copied()
                .unwrap_or(theme.base);
            let style = Style::new().fg(fg).bg(bg);
            buf.set_string(off_x + cx, off_y + cy, "▀", style);
        }
    }
}

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

fn mondrian(cx: u32, cy: u32, cw: u32, h: u32) -> u32 {
    let zw = (cw / 4).max(1);
    let zx = cx / zw;
    let zy = cy / zw;
    let key = zx ^ zy.wrapping_mul(7) ^ h;
    key & 7
}

fn concentric(cx: u32, cy: u32, cw: u32, h: u32) -> u32 {
    let half = i32::try_from(cw / 2).unwrap_or(0);
    let dx = i32::try_from(cx).unwrap_or(0) - half;
    let dy = i32::try_from(cy).unwrap_or(0) - half;
    let r2 = dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy));
    let band = u32::try_from(r2).unwrap_or(0) / (cw.max(1) + 1);
    band ^ (h >> 16)
}

fn stripes(cx: u32, cy: u32, h: u32) -> u32 {
    let v = (cx + cy) & 7;
    v ^ (h >> 8)
}

fn hash(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(16_777_619);
    }
    h
}
