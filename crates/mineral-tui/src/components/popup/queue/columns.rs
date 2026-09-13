//! queue 表格的列规格,随浮层内宽选择展示的文本列。

use ratatui::layout::Constraint;
use ratatui::widgets::Cell;

/// queue 表格的列档,按浮层内宽选(见 [`QueueColumns::for_width`])。
#[derive(Clone, Copy)]
pub(super) enum QueueColumns {
    /// 宽档:♥ / title / artist / album / len,文本列比例 Fill(3:2:2)。
    Wide,

    /// 中档:♥ / title / artist / len,文本列比例 Fill(3:2)。
    Full,

    /// 窄档:♥ / title / len。
    Song,
}

impl QueueColumns {
    /// 内宽不足 46 格时只展示歌名;46 格起增加艺人,58 格起增加专辑。
    pub(super) fn for_width(width: u16) -> Self {
        if width < 46 {
            Self::Song
        } else if width < 58 {
            Self::Full
        } else {
            Self::Wide
        }
    }

    /// 表头单元格(与 [`Self::widths`] / 行组装的列集严格一致)。
    pub(super) fn header_cells(self) -> Vec<Cell<'static>> {
        let mut cells = vec![Cell::from(""), Cell::from("title")];
        if matches!(self, Self::Wide | Self::Full) {
            cells.push(Cell::from("artist"));
        }
        if matches!(self, Self::Wide) {
            cells.push(Cell::from("album"));
        }
        cells.push(Cell::from("len"));
        cells
    }

    /// 收藏和时长列定宽,文本列按比例分配剩余宽度。
    pub(super) fn widths(self) -> Vec<Constraint> {
        let love = Constraint::Length(LOVE_W);
        let len = Constraint::Length(6);
        match self {
            Self::Wide => vec![
                love,
                Constraint::Fill(3),
                Constraint::Fill(2),
                Constraint::Fill(2),
                len,
            ],
            Self::Full => vec![love, Constraint::Fill(3), Constraint::Fill(2), len],
            Self::Song => vec![love, Constraint::Fill(1), len],
        }
    }
}

/// 收藏 gutter 宽,与曲目表一致(♥ 字形恒占一格,像 vim signcolumn 不抖后续列)。
const LOVE_W: u16 = 1;

/// 标题列在收藏列之后;marquee 按此列的宽度裁切标题,调整列序时须同步。
pub(super) const TITLE_COL: usize = 1;

#[cfg(test)]
mod tests {
    use super::QueueColumns;

    /// 文本列档随浮层内宽递进:窄档只展示歌名,中档增加艺人,宽档再增加专辑。
    #[test]
    fn queue_columns_tiers_by_width() {
        assert!(
            matches!(QueueColumns::for_width(45), QueueColumns::Song),
            "45 退到只剩歌名"
        );
        assert!(
            matches!(QueueColumns::for_width(46), QueueColumns::Full),
            "46 起放得下 artist"
        );
        assert!(
            matches!(QueueColumns::for_width(57), QueueColumns::Full),
            "57 仍塞不进 album"
        );
        assert!(
            matches!(QueueColumns::for_width(58), QueueColumns::Wide),
            "58 起再放 album"
        );
    }

    /// 列集与列宽严格同长——两者错位会让某列的内容画到邻列的宽度里。
    #[test]
    fn header_and_widths_stay_in_lockstep() {
        for width in [40, 50, 80] {
            let cols = QueueColumns::for_width(width);
            assert_eq!(
                cols.header_cells().len(),
                cols.widths().len(),
                "内宽 {width} 档的表头与列宽必须同长"
            );
        }
    }

    /// 回归:`TITLE_COL` 必须真的指向 title 列。它错位时表头照常(Table 自己按 widths 排),
    /// 只有走 marquee 的标题内容被裁成邻列的宽度——极难从渲染快照上一眼看出。
    #[test]
    fn title_col_points_at_the_title_column() {
        for width in [40, 50, 80] {
            let cols = QueueColumns::for_width(width);
            let header = cols.header_cells();
            assert_eq!(
                header.len(),
                cols.widths().len(),
                "内宽 {width} 档列数应一致"
            );
            assert_eq!(
                format!("{:?}", header.get(super::TITLE_COL)),
                format!("{:?}", Some(&ratatui::widgets::Cell::from("title"))),
                "内宽 {width} 档的 TITLE_COL 应指向 title 列"
            );
        }
    }
}
