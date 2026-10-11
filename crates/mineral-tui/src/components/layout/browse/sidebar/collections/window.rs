//! 固定列名下的项目行投影，绘制、图片与菜单共用。

use std::ops::Range;

use ratatui::layout::Rect;

use crate::runtime::collection::CollectionRef;
use crate::runtime::scroll::list::ScrollList;
use crate::runtime::scroll::viewport::pin_cursor;

/// 表格列名不参与光标与滚动下标。
pub(super) struct CollectionWindow {
    /// 去掉边框后的表格区域，首行固定列名。
    pub(super) table: Rect,

    /// 列名下方可滚动的项目行。
    pub(super) body: Rect,

    /// 当前可见项目在过滤结果中的范围。
    pub(super) visible: Range<usize>,

    /// 动画期间钉在视口内的高亮行。
    pub(super) selected: Option<usize>,
}

impl CollectionWindow {
    /// 扣除两行边框与一行固定列名。
    pub(super) fn scroll_height(area: Rect) -> usize {
        usize::from(area.height.saturating_sub(3))
    }

    /// 只投影实际项目，不向滚动序列插入标题。
    pub(super) fn new(area: Rect, rows: &[CollectionRef<'_>], scroll: &ScrollList) -> Self {
        let table = Rect::new(
            area.x.saturating_add(1),
            area.y.saturating_add(1),
            area.width.saturating_sub(2),
            area.height.saturating_sub(2),
        );
        let body = Rect::new(
            table.x,
            table.y + u16::from(table.height > 0),
            table.width,
            table.height.saturating_sub(1),
        );
        let offset = scroll.offset(rows.len(), Self::scroll_height(area));
        let visible = offset
            ..offset
                .saturating_add(usize::from(body.height))
                .min(rows.len());
        let selected =
            (!visible.is_empty()).then(|| pin_cursor(scroll.sel(), offset, visible.len()));
        Self {
            table,
            body,
            visible,
            selected,
        }
    }

    /// 某个项目实际占用的字符行。
    pub(super) fn row_area(&self, index: usize) -> Option<Rect> {
        if !self.visible.contains(&index) {
            return None;
        }
        Some(Rect::new(
            self.body.x,
            self.body.y + u16::try_from(index - self.visible.start).ok()?,
            self.body.width,
            1,
        ))
    }
}
