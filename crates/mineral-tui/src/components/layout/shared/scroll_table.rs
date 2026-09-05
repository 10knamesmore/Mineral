//! 可滚动表格的视口裁剪与选中行定位；只构造当前屏幕需要的行。

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{StatefulWidget, Table, TableState};

use crate::runtime::scroll::list::{ScrollList, ScrollMotion};
use crate::runtime::scroll::viewport::pin_cursor;

/// 先按完整列表求视口，再构造可见行并渲染到 `area`。
///
/// `viewport` 是数据行数(由调用方按各自 chrome 算:bordered + header 列表 = `area.height - 3`,
/// 仅 header = `area.height - 1`)。`motion` 定推进(稳态实拍)/ 冻结(离屏合成 / morph)。
///
/// # Params:
///   - `buf`: 目标 buffer
///   - `area`: 表格渲染区(含其自身 block 边框,若有)
///   - `build_table`: 接收完整列表中的可见下标范围，构造对应的单行高度表格
///   - `list`: 该列表的光标 + 视口滚动态
///   - `len`: 列表总行数
///   - `viewport`: 视口数据行数
///   - `motion`: 视口推进语义
pub(crate) fn render_scroll_table<'a>(
    buf: &mut Buffer,
    area: Rect,
    build_table: impl FnOnce(Range<usize>) -> Table<'a>,
    list: &ScrollList,
    len: usize,
    viewport: usize,
    motion: ScrollMotion,
) {
    let offset = list.offset(len, viewport, motion);
    let visible = offset..offset.saturating_add(viewport).min(len);
    let table = build_table(visible);
    // Table 只含可见窗口，高亮下标也必须平移到窗口内；逻辑光标和滚动仍使用完整列表。
    let mut state = TableState::default().with_selected(Some(
        pin_cursor(list.sel(), offset, viewport).saturating_sub(offset),
    ));
    StatefulWidget::render(table, area, buf, &mut state);
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::{Constraint, Rect};
    use ratatui::style::{Color, Style};
    use ratatui::widgets::{Row, StatefulWidget, Table, TableState};

    use crate::runtime::scroll::list::{ScrollList, ScrollMotion};
    use crate::runtime::scroll::viewport::pin_cursor;

    use super::render_scroll_table;

    /// 裁剪表格与完整表格在滚动边界和缓动中逐格相同，且只请求一个视口的数据。
    #[test]
    fn windowed_rows_match_full_table_during_scroll() {
        let area = Rect::new(
            /*x*/ 0, /*y*/ 0, /*width*/ 24, /*height*/ 8,
        );
        let mut list = ScrollList::new();
        let len = 120;
        let viewport = usize::from(area.height.saturating_sub(1));
        let motion = ScrollMotion::Advancing {
            scrolloff: 2,
            glide_ticks: 4,
        };
        let table = |range: std::ops::Range<usize>| {
            Table::new(
                range.map(|index| Row::new(vec![format!("song {index}")])),
                [Constraint::Fill(1)],
            )
            .header(Row::new(vec!["title"]))
            .highlight_symbol("▌ ")
            .row_highlight_style(Style::new().fg(Color::Yellow))
        };
        for selected in [0, 55, 119, 1] {
            list.set_sel(selected);
            for _ in 0..8 {
                let reference = list.clone();
                let offset = reference.offset(len, viewport, motion);
                let mut full = Buffer::empty(area);
                let mut cursor = TableState::default()
                    .with_offset(offset)
                    .with_selected(Some(pin_cursor(selected, offset, viewport)));
                StatefulWidget::render(table(0..len), area, &mut full, &mut cursor);
                let mut windowed = Buffer::empty(area);
                render_scroll_table(
                    &mut windowed,
                    area,
                    |visible| {
                        assert!(visible.len() <= viewport, "不构造屏幕外的行");
                        table(visible)
                    },
                    &list,
                    len,
                    viewport,
                    motion,
                );
                assert_eq!(windowed, full, "选中 {selected} 时屏幕内容应一致");
            }
        }
    }
}
