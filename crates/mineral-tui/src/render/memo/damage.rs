//! 失效区域按行合并，避免相交矩形把局部变化扩成整屏。

use std::ops::Range;

use ratatui::layout::Rect;

/// 每行至多一段待重建的 cell；没有变化的行不参与合成或比较。
pub(super) struct Damage {
    /// 当前终端画布。
    area: Rect,

    /// 相对画布顶部的行；同一行的变化覆盖到左右两端之间。
    rows: Vec<Option<Range<u16>>>,
}

impl Damage {
    /// 创建没有失效区域的画布记录。
    pub(super) fn new(area: Rect) -> Self {
        Self {
            area,
            rows: vec![None; usize::from(area.height)],
        }
    }

    /// 扩大失效区域，返回覆盖范围是否增加；坐标限制在当前画布内。
    pub(super) fn add(&mut self, area: Rect) -> bool {
        let area = area.intersection(self.area);
        if area.is_empty() {
            return false;
        }
        let mut changed = false;
        for y in area.top()..area.bottom() {
            if let Some(row) = self.rows.get_mut(usize::from(y - self.area.y)) {
                let next = row.as_ref().map_or(area.x..area.right(), |old| {
                    old.start.min(area.x)..old.end.max(area.right())
                });
                changed |= row.as_ref() != Some(&next);
                *row = Some(next);
            }
        }
        changed
    }

    /// 判断组件是否接触失效区域。
    pub(super) fn intersects(&self, area: Rect) -> bool {
        self.rows().any(|(y, columns)| {
            y >= area.top()
                && y < area.bottom()
                && columns.start < area.right()
                && area.x < columns.end
        })
    }

    /// 返回需要重建的行段，保持屏幕从上到下的顺序。
    pub(super) fn rows(&self) -> impl Iterator<Item = (u16, Range<u16>)> + '_ {
        (self.area.top()..self.area.bottom())
            .zip(&self.rows)
            .filter_map(|(y, columns)| columns.clone().map(|columns| (y, columns)))
    }

    /// 转成缓冲区的线性区间；相邻区间合并，让宽字符失效可以跨连续行传播。
    pub(super) fn ranges(&self) -> Vec<Range<usize>> {
        let mut ranges = Vec::<Range<usize>>::new();
        for (y, columns) in self.rows() {
            let row = usize::from(y - self.area.y) * usize::from(self.area.width);
            let next = row + usize::from(columns.start - self.area.x)
                ..row + usize::from(columns.end - self.area.x);
            if let Some(previous) = ranges.last_mut()
                && previous.end == next.start
            {
                previous.end = next.end;
            } else {
                ranges.push(next);
            }
        }
        ranges
    }
}
