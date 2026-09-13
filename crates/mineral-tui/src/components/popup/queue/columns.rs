//! queue 表格的列规格:按浮层内宽选择文本列,Kitty 在标题前增加封面。

use ratatui::layout::Constraint;
use ratatui::widgets::Cell;

use crate::components::layout::shared::thumbnails::THUMBNAIL_COLUMNS;

/// 队列表格的可见列;表头、行文本、marquee 与封面共用这份规格。
#[derive(Clone, Copy)]
pub(super) struct QueueColumns {
    /// 内宽至少 46 格时展示艺人。
    pub(super) artist: bool,

    /// 内宽至少 58 格时展示专辑。
    pub(super) album: bool,

    /// 当前图片协议支持 Kitty 缩略图时,在标题前保留图片列。
    pub(super) thumbnails: bool,
}

impl QueueColumns {
    /// 内宽不足 46 格时只展示歌名;46 格起增加艺人,58 格起增加专辑。
    pub(super) fn for_width(width: u16) -> Self {
        Self {
            artist: width >= 46,
            album: width >= 58,
            thumbnails: false,
        }
    }

    /// 按图片引擎的当前能力启用封面列,缺图时仍保留这一列。
    pub(super) fn with_thumbnails(self, enabled: bool) -> Self {
        Self {
            thumbnails: enabled,
            ..self
        }
    }

    /// 标题列跟在收藏列与可选封面列之后,marquee 按此列的实际宽度裁切。
    pub(super) fn title_index(self) -> usize {
        1 + usize::from(self.thumbnails)
    }

    /// 表头单元格,与列宽和行组装使用同一列序。
    pub(super) fn header_cells(self) -> Vec<Cell<'static>> {
        let mut cells = vec![Cell::from("")];
        if self.thumbnails {
            cells.push(Cell::from(""));
        }
        cells.push(Cell::from("title"));
        if self.artist {
            cells.push(Cell::from("artist"));
        }
        if self.album {
            cells.push(Cell::from("album"));
        }
        cells.push(Cell::from("len"));
        cells
    }

    /// 收藏、封面和时长列定宽,文本列按比例分配剩余宽度。
    pub(super) fn widths(self) -> Vec<Constraint> {
        let mut widths = vec![Constraint::Length(1)];
        if self.thumbnails {
            widths.push(Constraint::Length(THUMBNAIL_COLUMNS));
        }
        widths.push(Constraint::Fill(if self.artist { 3 } else { 1 }));
        if self.artist {
            widths.push(Constraint::Fill(2));
        }
        if self.album {
            widths.push(Constraint::Fill(2));
        }
        widths.push(Constraint::Length(6));
        widths
    }
}

#[cfg(test)]
mod tests {
    use super::QueueColumns;

    /// 文本列随浮层内宽递进:窄档只展示歌名,中档增加艺人,宽档再增加专辑。
    #[test]
    fn queue_columns_tiers_by_width() {
        for (width, artist, album) in [
            (45, false, false),
            (46, true, false),
            (57, true, false),
            (58, true, true),
        ] {
            for thumbnails in [false, true] {
                let cols = QueueColumns::for_width(width).with_thumbnails(thumbnails);
                assert_eq!(
                    (cols.artist, cols.album),
                    (artist, album),
                    "内宽 {width} 的文本列不因封面开关改变"
                );
            }
        }
    }

    /// 列集与列宽严格同长,避免内容错位到邻列。
    #[test]
    fn header_and_widths_stay_in_lockstep() {
        for width in [40, 50, 80] {
            for thumbnails in [false, true] {
                let cols = QueueColumns::for_width(width).with_thumbnails(thumbnails);
                assert_eq!(
                    cols.header_cells().len(),
                    cols.widths().len(),
                    "内宽 {width} 档的表头与列宽必须同长"
                );
            }
        }
    }

    /// 两种协议下 marquee 都指向 title,不会把标题裁成图片列的宽度。
    #[test]
    fn title_col_points_at_the_title_column() {
        for width in [40, 50, 80] {
            for thumbnails in [false, true] {
                let cols = QueueColumns::for_width(width).with_thumbnails(thumbnails);
                let header = cols.header_cells();
                assert_eq!(
                    header.len(),
                    cols.widths().len(),
                    "内宽 {width} 档列数应一致"
                );
                assert_eq!(
                    format!("{:?}", header.get(cols.title_index())),
                    format!("{:?}", Some(&ratatui::widgets::Cell::from("title"))),
                    "内宽 {width} 档的 marquee 应指向 title 列"
                );
            }
        }
    }
}
