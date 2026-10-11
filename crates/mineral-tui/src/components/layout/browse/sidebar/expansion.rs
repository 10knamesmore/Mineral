//! 列表准备与绘制共用的字符格几何。

use ratatui::layout::Rect;

/// 普通表格的可滚动行区域，扣除边框和固定表头。
pub(super) fn body(area: Rect, header_rows: u16) -> Rect {
    Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1 + header_rows),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2 + header_rows),
    )
}
