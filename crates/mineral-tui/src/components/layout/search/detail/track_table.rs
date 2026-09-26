//! Search detail 曲目表的列布局与文本行装配。
//!
//! 固定列为 ♥ / title / len，artist / album 按上下文与面板宽度增减。
//! Kitty 支持缩略图时在 title 前预留图片列，由调用方在整行高亮后覆盖封面。

use ratatui::layout::Constraint;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Row};

use mineral_model::Song;

use crate::components::layout::shared::marquee::RowMarquee;
use crate::components::layout::shared::text::alias_span;
use crate::components::layout::shared::thumbnails::THUMBNAIL_COLUMNS;
use crate::render::color::lerp_color;
use crate::render::theme::Theme;
use crate::runtime::format::format_ms_opt;

/// 选中行整行高亮的前缀符（与 browse / results 同款）。
pub const HIGHLIGHT_SYMBOL: &str = "▌ ";

/// 低于此面板宽度砍掉 artist/album 两列，退到「只剩歌名」（与 browse library 同一阈值，
/// 保两表窄屏行为一致）。
const NARROW_W: u16 = 56;

/// 曲目表的中间可选列。`♥`/`title`/`len` 恒在，`artist`/`album` 按上下文增减。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TrackColumns {
    /// 是否含 artist 列。
    pub artist: bool,

    /// 是否含 album 列。
    pub album: bool,

    /// 当前协议支持 Kitty 缩略图时，在 title 前保留图片列。
    thumbnails: bool,
}

impl TrackColumns {
    /// 构造一组列选择。
    pub fn new(artist: bool, album: bool) -> Self {
        Self {
            artist,
            album,
            thumbnails: false,
        }
    }

    /// 按当前图片协议能力启用图片列，图片未就绪时仍保留该列。
    ///
    /// # Params:
    ///   - `enabled`: `ImageEngine::supports_thumbnails` 返回的当前能力
    pub(super) fn with_thumbnails(self, enabled: bool) -> Self {
        Self {
            thumbnails: enabled,
            ..self
        }
    }

    /// 返回 title 列下标，供 marquee 使用包含图片列的实际宽度。
    pub(super) fn title_index(self) -> usize {
        1 + usize::from(self.thumbnails)
    }

    /// 按面板宽度降级：窄于 [`NARROW_W`] 时砍掉 artist/album（响应式，不写死字符数）。
    pub fn for_width(self, width: u16) -> Self {
        if width < NARROW_W {
            Self {
                artist: false,
                album: false,
                ..self
            }
        } else {
            self
        }
    }

    /// 表头单元格（与 [`Self::widths`] / [`track_row`] 的列集严格一致）。
    pub fn header_cells(self) -> Vec<Cell<'static>> {
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

    /// 列宽约束：定宽小列用 Length，文本列用比例 Fill（title 在有中间列时占大头）。
    pub fn widths(self) -> Vec<Constraint> {
        let mut w = vec![Constraint::Length(1)];
        if self.thumbnails {
            w.push(Constraint::Length(THUMBNAIL_COLUMNS));
        }
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

/// 选中行整行高亮样式,`focus_permille` = 面板焦点度(千分比):满值 accent 亮(BOLD)、
/// `0` 退暗调(subtext,无 BOLD,示意光标仍在、可回位),中间值沿焦点环滑动 subtext→accent
/// 渐变(非 RGB 主题 lerp 降级半程二态,BOLD 阈值同在半程,两者同步切)。
/// results 列与 detail 面板共用,两侧失焦表现对称。
pub fn highlight_style(theme: &Theme, focus_permille: u16) -> Style {
    let fg = lerp_color(
        theme.subtext,
        theme.accent,
        u64::from(focus_permille),
        /*denom*/ 1000,
    );
    let style = Style::new().bg(theme.surface0).fg(fg);
    if focus_permille >= 500 {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

/// ♥ gutter：loved → `♥`(red)，否则空（恒占一格，像 vim signcolumn，不抖后续列）。
fn love_cell(loved: bool, theme: &Theme) -> Cell<'static> {
    if loved {
        Cell::from(Span::styled("♥", Style::new().fg(theme.red)))
    } else {
        Cell::from("")
    }
}

/// 把一首裸 [`Song`] 装配成曲目表的一行（纯文本，无搜索高亮）：
/// ♥ / [封面空列] / title / [artist 首位] / [album] / len。
///
/// # Params:
///   - `song`: 该行歌曲
///   - `loved`: 是否已收藏（♥）
///   - `cols`: 中间列选择
///   - `marquee`: title 溢出滚动接线(仅光标选中行 `Some`,其余行截断)
pub fn track_row(
    song: &Song,
    loved: bool,
    cols: TrackColumns,
    theme: &Theme,
    marquee: Option<RowMarquee<'_>>,
) -> Row<'static> {
    let mut title_spans = vec![Span::styled(song.name.clone(), Style::new().fg(theme.text))];
    title_spans.extend(alias_span(song.alias.as_deref(), theme.overlay));
    let title_cell = match marquee {
        Some(m) => Cell::from(
            m.ctx
                .line(title_spans, m.slot, &song.id.qualified(), m.title_w),
        ),
        None => Cell::from(Line::from(title_spans)),
    };
    let mut cells = vec![love_cell(loved, theme)];
    if cols.thumbnails {
        cells.push(Cell::from(""));
    }
    cells.push(title_cell);
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
    cells.push(Cell::from(Line::from(format_ms_opt(song.duration_ms))));
    let row = Row::new(cells);
    if song.unavailable {
        row.style(theme.unavailable_row())
    } else {
        row
    }
}
