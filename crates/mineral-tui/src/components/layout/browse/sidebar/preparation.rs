//! 列表右边框的位置轨道。

use ratatui::layout::Rect;

/// 列表右边框的 minimap 轨道，包含表头与底部计数行。
pub(super) fn minimap_track(area: Rect) -> Rect {
    Rect::new(
        area.right().saturating_sub(1),
        area.y.saturating_add(1),
        area.width.min(1),
        area.height.saturating_sub(2),
    )
}
