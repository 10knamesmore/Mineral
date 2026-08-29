//! 把一段图形控制序列绑定到目标区域首 cell。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// 由首 cell 驱动、其余 cell 跳过 diff 的终端图片。
pub(super) struct CellImage {
    /// 写入目标区域首 cell 的完整控制序列。
    data: String,
}

impl CellImage {
    /// 构造首 cell 驱动的终端图片。
    pub(super) const fn new(data: String) -> Self {
        Self { data }
    }

    /// 写入控制序列并保护其覆盖的 cell 不被普通文本覆写。
    pub(super) fn render(&self, area: Rect, buffer: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        if let Some(cell) = buffer.cell_mut((area.left(), area.top())) {
            cell.set_symbol(&self.data);
        }
        let mut first = true;
        for row in area.top()..area.bottom() {
            for column in area.left()..area.right() {
                if first {
                    first = false;
                } else if let Some(cell) = buffer.cell_mut((column, row)) {
                    cell.set_skip(true);
                }
            }
        }
    }
}
