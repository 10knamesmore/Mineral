//! queue 表格的列规格:文本档位(随浮层内宽)+ 序号列宽(随队列规模)。

use ratatui::layout::Constraint;
use ratatui::widgets::Cell;

/// queue 表格的列档,按浮层内宽选(见 [`QueueCols::for_width`])。
///
/// 列序与曲目表(`search/detail/track_table`)对齐:♥ gutter 在最前,`#` 次之。
#[derive(Clone, Copy)]
pub(super) enum QueueCols {
    /// 宽档:♥ / # / title / artist / album / len,文本列比例 Fill(3:2:2)。
    Wide,

    /// 中档:♥ / # / title / artist / len,文本列比例 Fill(3:2)—— album 放不下。
    Full,

    /// 窄档:♥ / # / title / len —— artist 也放不下,退到「歌本身」。
    Song,
}

impl QueueCols {
    /// 按浮层内宽 `width` 选档。阈值在原有 44 / 56 上各加 2,补偿新增的收藏列——
    /// 不加会让窄档 title 比改动前少 2 格。
    fn for_width(width: u16) -> Self {
        if width < 46 {
            Self::Song
        } else if width < 58 {
            Self::Full
        } else {
            Self::Wide
        }
    }

    /// 表头单元格(与 [`Self::widths`] / 行组装的列集严格一致)。
    fn header_cells(self) -> Vec<Cell<'static>> {
        let mut cells = vec![Cell::from(""), Cell::from("#"), Cell::from("title")];
        if matches!(self, Self::Wide | Self::Full) {
            cells.push(Cell::from("artist"));
        }
        if matches!(self, Self::Wide) {
            cells.push(Cell::from("album"));
        }
        cells.push(Cell::from("len"));
        cells
    }

    /// 列宽约束:♥ / `#`(宽由 `index_w` 传入)/ len 定宽,文本列比例 Fill。
    fn widths(self, index_w: u16) -> Vec<Constraint> {
        let lead = [Constraint::Length(LOVE_W), Constraint::Length(index_w)];
        let len = Constraint::Length(6);
        match self {
            Self::Wide => lead
                .into_iter()
                .chain([
                    Constraint::Fill(3),
                    Constraint::Fill(2),
                    Constraint::Fill(2),
                    len,
                ])
                .collect(),
            Self::Full => lead
                .into_iter()
                .chain([Constraint::Fill(3), Constraint::Fill(2), len])
                .collect(),
            Self::Song => lead.into_iter().chain([Constraint::Fill(1), len]).collect(),
        }
    }
}

/// 收藏 gutter 宽,与曲目表一致(♥ 字形恒占一格,像 vim signcolumn 不抖后续列)。
const LOVE_W: u16 = 1;

/// 标题列在列序里的下标(♥ / `#` 之后)。marquee 要按这一列的宽度裁标题,取错列会把
/// 标题裁成邻列的宽度——加减列时必须同步。
pub(super) const TITLE_COL: usize = 2;

/// 序号列宽,随队列长度自适应。
///
/// `#` 列宽随规模伸缩:避免定宽把宽下标截断(定 3 宽时 `1234` 会被截成 `123`),也不让
/// 小队列白占列。阈值按队列**数量**取:≤999 首 3 宽,超过 4 宽。下标上界由 server 端队列
/// 长度(≤9999,0-based 故最大 9998)保证落在 4 位内,显示层照实渲染、不再另钳。
#[derive(Clone, Copy)]
struct IndexCol {
    /// 本帧该列的字符宽(3 或 4)。
    width: u16,
}

impl IndexCol {
    /// 按队列长度选列宽:≤999 首 3 宽,超过 4 宽。
    fn for_len(len: usize) -> Self {
        Self {
            width: if len <= 999 { 3 } else { 4 },
        }
    }

    /// 本列字符宽。
    fn width(self) -> u16 {
        self.width
    }
}

/// queue 表格的完整列规格:文本档位 + 序号列尺寸。
///
/// 把「按浮层内宽选的文本档」与「按队列长度选的序号列」收成一份规格随行传递,
/// 让行组装只认一个列上下文,不必平铺两个来源不同的入参。
#[derive(Clone, Copy)]
pub(super) struct QueueColumns {
    /// 文本列档位(按浮层内宽选,见 [`QueueCols::for_width`])。
    pub(super) text: QueueCols,

    /// 序号列宽(按队列长度选,见 [`IndexCol::for_len`])。
    index: IndexCol,
}

impl QueueColumns {
    /// 按浮层内宽 `width` 与队列长度 `len` 选列规格。
    pub(super) fn resolve(width: u16, len: usize) -> Self {
        Self {
            text: QueueCols::for_width(width),
            index: IndexCol::for_len(len),
        }
    }

    /// 表头单元格(与 [`Self::widths`] / 行组装的列集严格一致)。
    pub(super) fn header_cells(self) -> Vec<Cell<'static>> {
        self.text.header_cells()
    }

    /// 列宽约束:序号列宽由 [`IndexCol`] 定,其余随文本档。
    pub(super) fn widths(self) -> Vec<Constraint> {
        self.text.widths(self.index.width())
    }
}

#[cfg(test)]
mod tests {
    use super::{IndexCol, QueueCols};

    /// 文本列档随浮层内宽三档递进:窄档只剩歌名,中档放得下 artist,宽档再放 album。
    /// 阈值较收藏列引入前各高 2,补偿那一列占去的宽度。
    #[test]
    fn queue_cols_tiers_by_width() {
        assert!(
            matches!(QueueCols::for_width(45), QueueCols::Song),
            "45 退到只剩歌名"
        );
        assert!(
            matches!(QueueCols::for_width(46), QueueCols::Full),
            "46 起放得下 artist"
        );
        assert!(
            matches!(QueueCols::for_width(57), QueueCols::Full),
            "57 仍塞不进 album"
        );
        assert!(
            matches!(QueueCols::for_width(58), QueueCols::Wide),
            "58 起再放 album"
        );
    }

    /// 序号列宽随队列长度自适应:≤999 首 3 宽,破千转 4 宽(避免 4 位下标被定宽截断)。
    #[test]
    fn index_col_width_adapts_to_len() {
        assert_eq!(IndexCol::for_len(0).width(), 3, "空队列 3 宽");
        assert_eq!(IndexCol::for_len(999).width(), 3, "999 首仍 3 宽");
        assert_eq!(IndexCol::for_len(1000).width(), 4, "破千转 4 宽");
        assert_eq!(IndexCol::for_len(9999).width(), 4, "满队列(9999)4 宽");
    }

    /// 列集与列宽严格同长——两者错位会让某列的内容画到邻列的宽度里。
    #[test]
    fn header_and_widths_stay_in_lockstep() {
        for width in [40, 50, 80] {
            let cols = super::QueueColumns::resolve(width, /*len*/ 10);
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
            let cols = super::QueueColumns::resolve(width, /*len*/ 10);
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
