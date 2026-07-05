//! 定长滑块纵向滚动条:滑块长只随内容/视口比例定、**不随滚动偏移变**。
//!
//! ratatui 内置 `Scrollbar` 的滑块长会随 position 波动,滚动时「一长一短蠕动」;
//! 这里的几何把长度钉死,到顶贴顶、到底滑块底边正好贴轨道底。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

use crate::render::theme::Theme;

/// 纵向滚动条滑块几何(纯函数,便于回归测试):返回 `(滑块顶端行偏移, 滑块长)`。
///
/// 滑块长只随 `viewport`/`total`(**不含 `off`**,故滚动时长度恒定、不蠕动);`off=0` 顶端=0
/// (贴顶)、`off=max_off`(=`total-viewport`)时 `顶端+长==track_len`(底边贴轨道底)。
/// `track_len`/`total` 为 0 → `(0, 0)`。
pub(crate) fn scrollbar_thumb(total: u16, viewport: u16, off: u16, track_len: u16) -> (u16, u16) {
    if track_len == 0 || total == 0 {
        return (0, 0);
    }
    // 滑块长 = viewport 占 total 的比例 × 轨道高,夹到 [1, 轨道高];只随内容/视口定、不随 off。
    let thumb_len = u16::try_from(u32::from(viewport) * u32::from(track_len) / u32::from(total))
        .unwrap_or(track_len)
        .clamp(1, track_len);
    let travel = track_len.saturating_sub(thumb_len);
    let max_off = total.saturating_sub(viewport);
    let thumb_top = if max_off == 0 {
        0
    } else {
        u16::try_from(u32::from(off) * u32::from(travel) / u32::from(max_off)).unwrap_or(travel)
    };
    (thumb_top, thumb_len)
}

/// 在 `area` 末列画定长滑块的纵向滚动条(几何见 [`scrollbar_thumb`])。仅溢出
/// (`total > viewport`)时调用。
pub(crate) fn draw_scrollbar(
    buf: &mut Buffer,
    area: Rect,
    total: u16,
    viewport: u16,
    off: u16,
    theme: &Theme,
) {
    let track_len = area.height;
    if track_len == 0 || area.width == 0 {
        return;
    }
    let (thumb_top, thumb_len) = scrollbar_thumb(total, viewport, off, track_len);
    let x = area.x.saturating_add(area.width).saturating_sub(1);
    let thumb = Style::new().fg(theme.subtext);
    let track = Style::new().fg(theme.overlay);
    for i in 0..track_len {
        let (sym, style) = if i >= thumb_top && i < thumb_top.saturating_add(thumb_len) {
            ("█", thumb)
        } else {
            ("│", track)
        };
        if let Some(cell) = buf.cell_mut((x, area.y.saturating_add(i))) {
            cell.set_symbol(sym).set_style(style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::scrollbar_thumb;

    /// 回归锁(用户实测两 bug):滚动条滑块长度恒定(不随 off「一长一短蠕动」)、到顶贴顶、
    /// **到底滑块底边正好贴轨道底**(此前到底了滑块还没到底、误导下面有内容)。
    #[test]
    fn scrollbar_thumb_constant_len_and_flush_bottom() {
        let (total, viewport, track) = (50u16, 10u16, 8u16);
        let max_off = total - viewport;
        let (top_0, len_0) = scrollbar_thumb(total, viewport, /*off*/ 0, track);
        let (_, len_mid) = scrollbar_thumb(total, viewport, max_off / 2, track);
        let (top_max, len_max) = scrollbar_thumb(total, viewport, max_off, track);
        // 长度恒定:三处一致 → 不蠕动。
        assert_eq!(len_0, len_mid, "滑块长度不随 off 变(中途)");
        assert_eq!(len_0, len_max, "滑块长度不随 off 变(到底)");
        // 到顶贴顶、到底贴底。
        assert_eq!(top_0, 0, "off=0 滑块贴顶");
        assert_eq!(
            top_max + len_max,
            track,
            "off=max 滑块底边贴轨道底(回归:到底滑块没到底)"
        );
    }

    /// 内容恰好不溢出(total==viewport):滑块满轨、贴顶(不会出现半截滑块)。
    #[test]
    fn scrollbar_thumb_fills_track_when_no_overflow() {
        let (top, len) = scrollbar_thumb(
            /*total*/ 8, /*viewport*/ 8, /*off*/ 0, /*track*/ 8,
        );
        assert_eq!((top, len), (0, 8), "不溢出时滑块满轨贴顶");
    }
}
