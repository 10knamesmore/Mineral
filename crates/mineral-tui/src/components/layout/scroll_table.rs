//! 可滚动列表表格的唯一渲染入口:给定 [`ScrollList`] 出 offset、按 `pin_cursor` 钳高亮行,
//! 组装 `TableState` 后渲染。
//!
//! 所有列表表(browse 歌单 / 曲目、queue、search 结果列 / detail)都经此渲染——把
//! 「`TableState::offset` + `pin_cursor` 高亮」这对**必须同时出现**的装配收成一处。
//! 杜绝再有谁用 `TableState::default().with_selected(..)` 漏掉 `with_offset`,让 ratatui
//! 每帧从 offset=0 重求最小可见滚动、把选中行钉死视口底边(focus 贴边)。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::{StatefulWidget, Table, TableState};

use crate::runtime::scroll::pin_cursor;
use crate::runtime::scroll_list::{ScrollList, ScrollMotion};

/// 把 `table` 按 `list` 的视口滚动态渲染到 `area`。
///
/// `viewport` 是数据行数(由调用方按各自 chrome 算:bordered + header 列表 = `area.height - 3`,
/// 仅 header = `area.height - 1`)。`motion` 定推进(稳态实拍)/ 冻结(离屏合成 / morph)。
///
/// # Params:
///   - `buf`: 目标 buffer
///   - `area`: 表格渲染区(含其自身 block 边框,若有)
///   - `table`: 已配好列 / 表头 / 高亮样式的表
///   - `list`: 该列表的光标 + 视口滚动态
///   - `len`: 列表总行数
///   - `viewport`: 视口数据行数
///   - `motion`: 视口推进语义
pub(crate) fn render_scroll_table(
    buf: &mut Buffer,
    area: Rect,
    table: Table<'_>,
    list: &ScrollList,
    len: usize,
    viewport: usize,
    motion: ScrollMotion,
) {
    let offset = list.offset(len, viewport, motion);
    let mut state = TableState::default()
        .with_offset(offset)
        .with_selected(Some(pin_cursor(list.sel(), offset, viewport)));
    StatefulWidget::render(table, area, buf, &mut state);
}
