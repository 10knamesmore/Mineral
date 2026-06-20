//! 曲目表的列布局与行装配：browse library 表与 search detail 曲目表共用一套列集，
//! 杜绝两处各写一份 ♥/#/title/artist/album/len 而风格漂移。
//!
//! 固定列：♥ gutter（loved 标记）+ # （在播 ♫ / 0 起序号）+ title …… + len；
//! 中间 artist / album 两列按上下文（[`TrackColumns`]）与面板宽度增减。

use ratatui::layout::Constraint;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Row};

use mineral_model::Song;

use super::search_panel::format_duration;
use crate::render::theme::Theme;

/// 选中行整行高亮的前缀符（与 browse / results 同款）。
pub const HIGHLIGHT_SYMBOL: &str = "▌ ";

/// 低于此面板宽度砍掉 artist/album 两列，退到「只剩歌名」（与 browse library 同一阈值，
/// 保两表窄屏行为一致）。
const NARROW_W: u16 = 56;

/// 曲目表的中间可选列。`♥`/`#`/`title`/`len` 恒在，`artist`/`album` 按上下文增减。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TrackColumns {
    /// 是否含 artist 列。
    pub artist: bool,

    /// 是否含 album 列。
    pub album: bool,
}

impl TrackColumns {
    /// 构造一组列选择。
    pub fn new(artist: bool, album: bool) -> Self {
        Self { artist, album }
    }

    /// 按面板宽度降级：窄于 [`NARROW_W`] 时砍掉 artist/album（响应式，不写死字符数）。
    pub fn for_width(self, width: u16) -> Self {
        if width < NARROW_W {
            Self::new(/*artist*/ false, /*album*/ false)
        } else {
            self
        }
    }

    /// 表头单元格（与 [`Self::widths`] / [`track_row`] 的列集严格一致）。
    pub fn header_cells(self) -> Vec<Cell<'static>> {
        let mut cells = vec![Cell::from(""), Cell::from("#"), Cell::from("title")];
        if self.artist {
            cells.push(Cell::from("artist"));
        }
        if self.album {
            cells.push(Cell::from("album"));
        }
        cells.push(Cell::from("len"));
        cells
    }

    /// 列宽约束：定宽小列用 Length，文本列用比例 Fill（title 在有中间列时占大头）。
    pub fn widths(self) -> Vec<Constraint> {
        let mut w = vec![Constraint::Length(1), Constraint::Length(4)];
        if self.artist || self.album {
            w.push(Constraint::Fill(3));
            if self.artist {
                w.push(Constraint::Fill(2));
            }
            if self.album {
                w.push(Constraint::Fill(2));
            }
        } else {
            w.push(Constraint::Fill(1));
        }
        w.push(Constraint::Length(6));
        w
    }
}

/// 表头 Row（subtext + BOLD，与 browse / results 同款）。
pub fn header_row(cols: TrackColumns, theme: &Theme) -> Row<'static> {
    Row::new(cols.header_cells()).style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD))
}

/// 选中行整行高亮样式（bg surface0 + fg accent + BOLD）。
pub fn highlight_style(theme: &Theme) -> Style {
    Style::new()
        .bg(theme.surface0)
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD)
}

/// ♥ gutter：loved → `♥`(red)，否则空（恒占一格，像 vim signcolumn，不抖后续列）。
fn love_cell(loved: bool, theme: &Theme) -> Cell<'static> {
    if loved {
        Cell::from(Span::styled("♥", Style::new().fg(theme.red)))
    } else {
        Cell::from("")
    }
}

/// `#` 列：在播 → `♫`(accent)，否则 0 起序号。
fn num_cell(idx: usize, is_current: bool, theme: &Theme) -> Cell<'static> {
    if is_current {
        Cell::from(Span::styled("♫", Style::new().fg(theme.accent)))
    } else {
        Cell::from(format!("{idx}"))
    }
}

/// 把一首裸 [`Song`] 装配成曲目表的一行（纯文本，无搜索高亮）：
/// ♥ / #（在播 ♫ / 序号）/ title / [artist 首位] / [album] / len。
///
/// # Params:
///   - `song`: 该行歌曲
///   - `idx`: 0 起行号（在播则被 `♫` 取代）
///   - `loved`: 是否已收藏（♥）
///   - `is_current`: 是否当前在播（♫）
///   - `cols`: 中间列选择
pub fn track_row(
    song: &Song,
    idx: usize,
    loved: bool,
    is_current: bool,
    cols: TrackColumns,
    theme: &Theme,
) -> Row<'static> {
    let mut cells = vec![
        love_cell(loved, theme),
        num_cell(idx, is_current, theme),
        Cell::from(Span::styled(song.name.clone(), Style::new().fg(theme.text))),
    ];
    if cols.artist {
        let artist = song
            .artists
            .first()
            .map(|a| a.name.clone())
            .unwrap_or_default();
        cells.push(Cell::from(Span::styled(
            artist,
            Style::new().fg(theme.subtext),
        )));
    }
    if cols.album {
        let album = song
            .album
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_default();
        cells.push(Cell::from(Span::styled(
            album,
            Style::new().fg(theme.overlay),
        )));
    }
    cells.push(Cell::from(Line::from(format_duration(song.duration_ms))));
    Row::new(cells)
}

#[cfg(test)]
mod tests {
    use super::TrackColumns;

    /// 列集不变量：表头单元格数必须等于列宽约束数（否则 ratatui 错位）。覆盖四种组合。
    #[test]
    fn header_and_widths_stay_aligned() {
        for cols in [
            TrackColumns::new(/*artist*/ true, /*album*/ true),
            TrackColumns::new(true, false),
            TrackColumns::new(false, true),
            TrackColumns::new(false, false),
        ] {
            assert_eq!(
                cols.header_cells().len(),
                cols.widths().len(),
                "{cols:?}: 表头列数应与列宽数一致"
            );
        }
    }

    /// 列数随中间列增减：base 4 列(♥/#/title/len) + artist + album。
    #[test]
    fn column_count_tracks_flags() {
        assert_eq!(TrackColumns::new(false, false).widths().len(), 4);
        assert_eq!(TrackColumns::new(true, false).widths().len(), 5);
        assert_eq!(TrackColumns::new(false, true).widths().len(), 5);
        assert_eq!(TrackColumns::new(true, true).widths().len(), 6);
    }

    /// 窄屏降级：宽度 < 56 砍掉 artist/album，≥ 56 原样保留。
    #[test]
    fn for_width_drops_middle_columns_when_narrow() {
        let full = TrackColumns::new(true, true);
        assert_eq!(full.for_width(80), full, "够宽原样");
        assert_eq!(
            full.for_width(40),
            TrackColumns::new(false, false),
            "窄屏退到只剩歌名"
        );
        // 边界：恰 56 保留、55 降级。
        assert_eq!(full.for_width(56), full);
        assert_eq!(full.for_width(55), TrackColumns::new(false, false));
    }
}
