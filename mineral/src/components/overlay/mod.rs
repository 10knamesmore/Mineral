//! 浮层 / modal 渲染:queue / quit confirm 等。
//!
//! 全部用 [`Flex::Center`] + Percentage + min/max clamp 计算位置,
//! 不写死字符尺寸。

pub mod queue;

use ratatui::layout::Rect;

/// 居中 + 钳制的尺寸计算。
///
/// `pct_w` / `pct_h` 是相对 `area` 的百分比,`min_*` / `max_*` 是绝对字符
/// 下/上限。最终再被 `area` 自身大小钳制,保证不溢出。
pub fn centered_rect(
    area: Rect,
    pct_w: u16,
    pct_h: u16,
    min_w: u16,
    min_h: u16,
    max_w: u16,
    max_h: u16,
) -> Rect {
    let w_target = u32::from(area.width) * u32::from(pct_w) / 100;
    let h_target = u32::from(area.height) * u32::from(pct_h) / 100;
    let w = u16::try_from(w_target.clamp(u32::from(min_w), u32::from(max_w))).unwrap_or(min_w);
    let h = u16::try_from(h_target.clamp(u32::from(min_h), u32::from(max_h))).unwrap_or(min_h);
    let w = w.min(area.width);
    let h = h.min(area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w, h)
}
