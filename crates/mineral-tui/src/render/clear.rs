//! 清理覆盖区域及左边界截断的双格字符，供浮层、通知与布局裁剪共用。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

/// 清空绘制区域；左邻格若是被截断的宽字符首格，只擦字符，保留底层样式。
pub(crate) struct Clear;

impl Widget for Clear {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = area.intersection(buf.area);
        if area.is_empty() {
            return;
        }
        if let Some(left) = area.x.checked_sub(1) {
            for y in area.top()..area.bottom() {
                if let Some(cell) = buf.cell_mut((left, y))
                    && cell.symbol().width() > 1
                {
                    // 首格留在区域外时，Buffer::diff 会跳过它占用的边框格。
                    cell.set_symbol(" ");
                }
            }
        }
        ratatui::widgets::Clear.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::widgets::Widget;

    use super::Clear;

    /// 截断的首格保留全部样式，清理区域外的完整字符不变。
    #[test]
    fn clear_removes_cut_wide_character_without_erasing_background() {
        let mut buf = Buffer::empty(Rect::new(4, 3, 12, 3));
        let style = Style::new()
            .fg(Color::Yellow)
            .bg(Color::Blue)
            .underline_color(Color::Red)
            .add_modifier(Modifier::BOLD);
        buf.set_string(4, 4, "甲乙丙丁戊己", style);
        let mut expected = buf.clone();
        if let Some(cell) = expected.cell_mut((6, 4)) {
            cell.set_symbol(" ");
        }
        ratatui::widgets::Clear.render(Rect::new(7, 4, 3, 1), &mut expected);

        Clear.render(Rect::new(7, 4, 3, 1), &mut buf);

        assert_eq!(buf.cell((6, 4)).map(Cell::symbol), Some(" "));
        assert_eq!(buf.cell((6, 4)).map(Cell::style), Some(style));
        assert_eq!(buf, expected, "完整字符、区域外格子与背景保持原样");
    }

    /// ASCII、完整双格字符、非零缓冲原点和空区域不额外擦除左邻格。
    #[test]
    fn clear_preserves_adjacent_complete_characters_and_empty_areas() {
        let mut original = Buffer::empty(Rect::new(4, 3, 12, 1));
        original.set_string(4, 3, "甲a乙b丙c丁d", Style::new());
        for area in [
            Rect::new(7, 3, 3, 1),
            Rect::new(6, 3, 1, 1),
            Rect::new(4, 3, 2, 1),
            Rect::new(8, 3, 0, 1),
            Rect::new(8, 3, 2, 0),
        ] {
            let mut expected = original.clone();
            ratatui::widgets::Clear.render(area, &mut expected);
            let mut actual = original.clone();
            Clear.render(area, &mut actual);
            assert_eq!(actual, expected, "area={area:?}");
        }
    }
}
