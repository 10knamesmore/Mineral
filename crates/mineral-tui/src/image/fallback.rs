//! 使用进程内随机且稳定的图案绘制主题化程序封面。

use std::sync::OnceLock;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use super::geometry::square_cells;
use crate::render::theme::Theme;

/// 在目标区域内绘制一张随机半块字符封面。
///
/// 图案在首次使用时随机生成，随后在整个进程内保持不变，避免连续渲染时闪烁。
///
/// # Params:
///   - `buffer`: 当前屏幕或离屏缓冲
///   - `area`: 可用的 cell 区域
///   - `theme`: 当前生效的 TUI 主题
pub(crate) fn render_random(buffer: &mut Buffer, area: Rect, theme: &Theme) {
    let square = square_cells(area);
    if square.width == 0 || square.height == 0 {
        return;
    }
    let palette = [
        theme.crust,
        theme.surface0,
        theme.overlay,
        theme.subtext,
        theme.text,
        theme.accent,
        theme.accent_2,
        theme.peach,
    ];
    let pattern_id = *PATTERN_ID.get_or_init(rand::random);
    for row in 0..square.height {
        for column in 0..square.width {
            let upper = palette_color(
                &palette,
                pattern_id,
                column,
                row.saturating_mul(2),
                square.width,
                theme.text,
            );
            let lower = palette_color(
                &palette,
                pattern_id,
                column,
                row.saturating_mul(2).saturating_add(1),
                square.width,
                theme.base,
            );
            buffer.set_string(
                square.x.saturating_add(column),
                square.y.saturating_add(row),
                "▀",
                Style::new().fg(upper).bg(lower),
            );
        }
    }
}

/// 本进程所有 fallback 共用的随机图案身份。
static PATTERN_ID: OnceLock<u32> = OnceLock::new();

/// 返回一个逻辑像素对应的主题色。
fn palette_color(
    palette: &[Color; 8],
    pattern_id: u32,
    x: u16,
    y: u16,
    width: u16,
    fallback: Color,
) -> Color {
    palette
        .get(usize::from(pattern(pattern_id, x, y, width)) & 7)
        .copied()
        .unwrap_or(fallback)
}

/// 按随机身份高位选择图案，并返回 8 色调色盘索引。
fn pattern(pattern_id: u32, x: u16, y: u16, width: u16) -> u8 {
    let x = u32::from(x);
    let y = u32::from(y);
    let width = u32::from(width).max(1);
    let value = match (pattern_id >> 24) & 0x03 {
        0 => horizon(y, width, pattern_id),
        1 => mondrian(x, y, width, pattern_id),
        2 => concentric(x, y, width, pattern_id),
        _ => stripes(x, y, pattern_id),
    };
    u8::try_from(value & 0xff).unwrap_or(0)
}

/// 返回上下分区图案的调色盘索引。
fn horizon(y: u32, width: u32, pattern_id: u32) -> u32 {
    if y < width / 2 {
        5 + ((y ^ pattern_id) & 1)
    } else {
        1 + ((y ^ pattern_id) & 1)
    }
}

/// 返回矩形分块图案的调色盘索引。
fn mondrian(x: u32, y: u32, width: u32, pattern_id: u32) -> u32 {
    let block_width = (width / 4).max(1);
    let block_x = x / block_width;
    let block_y = y / block_width;
    (block_x ^ block_y.wrapping_mul(7) ^ pattern_id) & 7
}

/// 返回同心环图案的调色盘索引。
fn concentric(x: u32, y: u32, width: u32, pattern_id: u32) -> u32 {
    let half = i32::try_from(width / 2).unwrap_or(0);
    let dx = i32::try_from(x).unwrap_or(0) - half;
    let dy = i32::try_from(y).unwrap_or(0) - half;
    let radius = dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy));
    let band = u32::try_from(radius).unwrap_or(0) / (width + 1);
    band ^ (pattern_id >> 16)
}

/// 返回斜条纹图案的调色盘索引。
fn stripes(x: u32, y: u32, pattern_id: u32) -> u32 {
    ((x + y) & 7) ^ (pattern_id >> 8)
}
