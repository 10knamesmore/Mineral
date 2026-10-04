//! 列表准备与绘制共用的字符格几何。

use ratatui::layout::Rect;

/// 数据行区域，准备与绘制共用。
pub(super) fn body(area: Rect) -> Rect {
    let body = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(2),
        area.width.saturating_sub(2),
        area.height.saturating_sub(3),
    );
    body
}
