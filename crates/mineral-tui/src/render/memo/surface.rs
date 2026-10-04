//! 终端已知 cell 与字符宽度；只比较失效区间及其宽字符尾部。

use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr as _;

use super::damage::Damage;

/// cell 内容与由其 symbol 派生的列宽一起更新，颜色变化不重复解码 UTF-8。
#[derive(Clone)]
struct ScreenCell {
    /// 最近合成的终端 cell。
    content: Cell,

    /// 与 content.symbol 对应的 Unicode 列宽。
    width: usize,
}

/// 保留画布的输出参照；终端 I/O 失败后调用方退出，不继续沿用本次参照。
pub(super) struct Surface {
    /// 与实际终端画布相同的坐标范围。
    area: Rect,

    /// 屏幕顺序的 cell 及其派生宽度。
    cells: Vec<ScreenCell>,
}

impl Surface {
    /// 清屏或 resize 后，从终端空白状态建立输出参照。
    pub(super) fn new(area: Rect) -> Self {
        let content = Cell::default();
        let width = content.symbol().width();
        Self {
            area,
            cells: vec![
                ScreenCell { content, width };
                usize::from(area.width) * usize::from(area.height)
            ],
        }
    }

    /// 返回输出参照的几何。
    pub(super) const fn area(&self) -> Rect {
        self.area
    }

    /// 复用 ratatui 的 skip、宽字符覆盖和失效尾部规则，只扫描本次变化区间。
    /// 每个区间继承左邻 cell 的占宽；右端继续处理尚未恢复的宽字符尾部。
    pub(super) fn diff(&mut self, current: &Buffer, damage: &Damage) -> Vec<usize> {
        let mut updates = Vec::new();
        let mut scanned_until = 0;
        for range in damage.ranges() {
            let mut index = range.start.max(scanned_until);
            let mut to_skip = index
                .checked_sub(1)
                .and_then(|previous| current.content.get(previous).zip(self.cells.get(previous)))
                .map_or(0, |(cell, previous)| {
                    if cell.symbol() == previous.content.symbol() {
                        previous.width
                    } else {
                        cell.symbol().width()
                    }
                })
                .saturating_sub(1);
            let mut invalidated = 0;
            while index < range.end || invalidated > 0 {
                let Some((cell, previous)) =
                    current.content.get(index).zip(self.cells.get_mut(index))
                else {
                    break;
                };
                let width = if cell.symbol() == previous.content.symbol() {
                    previous.width
                } else {
                    cell.symbol().width()
                };
                let changed = cell != &previous.content;
                if !cell.skip && (changed || invalidated > 0) && to_skip == 0 {
                    updates.push(index);
                }
                to_skip = width.saturating_sub(1);
                invalidated = invalidated.max(width.max(previous.width)).saturating_sub(1);
                if changed {
                    previous.content.clone_from(cell);
                    previous.width = width;
                }
                index += 1;
            }
            scanned_until = index;
        }
        updates
    }

    /// 借出本次要交给 Backend 的 cell；不再次复制 symbol 或构造文本。
    pub(super) fn updates<'a>(
        &'a self,
        indices: &'a [usize],
    ) -> impl Iterator<Item = (u16, u16, &'a Cell)> + 'a {
        indices.iter().filter_map(|&index| {
            let cell = self.cells.get(index)?;
            let width = usize::from(self.area.width);
            let x = u16::try_from(index % width).ok()? + self.area.x;
            let y = u16::try_from(index / width).ok()? + self.area.y;
            Some((x, y, &cell.content))
        })
    }
}
