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

use crate::components::layout::shared::marquee::RowMarquee;
use crate::components::layout::shared::text::alias_span;
use crate::render::color::lerp_color;
use crate::render::theme::Theme;
use crate::runtime::format::format_ms_opt;

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
///   - `marquee`: title 溢出滚动接线(仅光标选中行 `Some`,其余行截断)
pub fn track_row(
    song: &Song,
    idx: usize,
    loved: bool,
    is_current: bool,
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
    let mut cells = vec![
        love_cell(loved, theme),
        num_cell(idx, is_current, theme),
        title_cell,
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
    cells.push(Cell::from(Line::from(format_ms_opt(song.duration_ms))));
    let row = Row::new(cells);
    if song.unavailable {
        row.style(theme.unavailable_row())
    } else {
        row
    }
}

#[cfg(test)]
mod tests {
    use super::TrackColumns;

    /// unavailable 行整行 DIM 降权、正常行不带;样式断言(文本快照显不出 modifier)。
    #[test]
    fn unavailable_row_is_dimmed() -> color_eyre::Result<()> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::style::Modifier;
        use ratatui::widgets::Table;

        use crate::render::theme::Theme;

        let theme = Theme::default();
        let normal = mineral_test::song("ok");
        let mut grey = mineral_test::song("grey");
        grey.unavailable = true;
        let cols = TrackColumns::new(/*artist*/ false, /*album*/ false);
        let rows = vec![
            super::track_row(
                &normal, 0, /*loved*/ false, /*is_current*/ false, cols, &theme,
                /*marquee*/ None,
            ),
            super::track_row(
                &grey, 1, /*loved*/ false, /*is_current*/ false, cols, &theme,
                /*marquee*/ None,
            ),
        ];
        let mut t = Terminal::new(TestBackend::new(40, 4))?;
        t.draw(|f| f.render_widget(Table::new(rows, cols.widths()), f.area()))?;
        let buf = t.backend().buffer();
        // 无表头:y=0 正常行、y=1 灰行;x=8 落在 title 列内。
        let dim_at = |y: u16| buf.cell((8, y)).map(|c| c.modifier.contains(Modifier::DIM));
        assert_eq!(dim_at(0), Some(false), "正常行不得 DIM");
        assert_eq!(dim_at(1), Some(true), "unavailable 行应整行 DIM");
        Ok(())
    }

    /// 带别名的歌名后缀 ` (alias)` 为 overlay 暗色,主名保持 text 色。
    #[test]
    fn alias_suffix_is_dim() -> color_eyre::Result<()> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::widgets::Table;

        use crate::render::theme::Theme;

        let theme = Theme::default();
        // 真实译名样本:迷星叫 / 叫喊迷星(分隔符 / 括号已由 alias_span 单测锁定,此处验渲染颜色)。
        let song = mineral_test::aliased_song();
        let cols = TrackColumns::new(/*artist*/ false, /*album*/ false);
        let rows = vec![super::track_row(
            &song, 0, /*loved*/ false, /*is_current*/ false, cols, &theme,
            /*marquee*/ None,
        )];
        let mut t = Terminal::new(TestBackend::new(40, 2))?;
        t.draw(|f| f.render_widget(Table::new(rows, cols.widths()), f.area()))?;
        let buf = t.backend().buffer();
        let line = (0..buf.area.width)
            .filter_map(|x| buf.cell((x, 0)).map(ratatui::buffer::Cell::symbol))
            .collect::<String>();
        // 别名内容渲染出来(CJK 双宽后随空补位 cell,去空格再验内容存在)。
        assert!(
            line.replace(' ', "").contains("迷星叫(Mayoiuta)"),
            "title 应带别名内容: {line}"
        );
        let fg_of = |ch: &str| -> Option<ratatui::style::Color> {
            (0..buf.area.width)
                .find_map(|x| buf.cell((x, 0)).filter(|c| c.symbol() == ch).map(|c| c.fg))
        };
        assert_eq!(fg_of("("), Some(theme.overlay), "别名后缀应为 overlay 暗色");
        assert_eq!(fg_of("迷"), Some(theme.text), "主名应保持 text 色");
        Ok(())
    }

    /// 传 marquee 的行:title 溢出按相位滚动,推进拍数后从对应列起显示;
    /// title 列宽经 resolve_column_widths 解算,与 Table 实际列边界一致。
    #[test]
    fn marquee_row_title_scrolls() -> color_eyre::Result<()> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::widgets::Table;

        use crate::components::layout::shared::marquee::{
            MarqueeCtx, RowMarquee, resolve_column_widths,
        };
        use crate::render::theme::Theme;
        use crate::runtime::marquee::{Marquees, Slot};
        use crate::test_support::{song, with_name};

        let theme = Theme::default();
        let long = with_name(song("1"), "abcdefghijklmnopqrstuvwxyz0123456789");
        let cols = TrackColumns::new(/*artist*/ false, /*album*/ false);
        let widths = cols.widths();
        // 测试渲染无 TableState 选中 → selection_w 0。
        let title_w = resolve_column_widths(/*total_w*/ 40, &widths, /*selection_w*/ 0)
            .get(2)
            .copied()
            .ok_or_else(|| color_eyre::eyre::eyre!("缺 title 列"))?;
        let mut mq = Marquees::test_loop(/*step_ticks*/ 1, /*pause_ticks*/ 0);
        let render = |mq: &Marquees| -> color_eyre::Result<String> {
            let ctx = MarqueeCtx {
                marquees: mq,
                gap: "  ✦  ",
                gap_style: ratatui::style::Style::new(),
                fade_to: ratatui::style::Color::Reset,
                fade_cols: 3,
            };
            let rows = vec![super::track_row(
                &long,
                0,
                /*loved*/ false,
                /*is_current*/ false,
                cols,
                &theme,
                Some(RowMarquee {
                    ctx: &ctx,
                    slot: Slot::BrowseSelected,
                    title_w,
                }),
            )];
            let mut t = Terminal::new(TestBackend::new(40, 1))?;
            t.draw(|f| f.render_widget(Table::new(rows, widths.clone()), f.area()))?;
            let buf = t.backend().buffer();
            Ok((0..buf.area.width)
                .filter_map(|x| buf.cell((x, 0)).map(ratatui::buffer::Cell::symbol))
                .collect::<String>())
        };
        assert!(render(&mq)?.contains("abcdef"), "建档帧应从歌名开头显示");
        for _ in 0..4 {
            mq.tick();
        }
        let scrolled = render(&mq)?;
        assert!(
            scrolled.contains("efghij") && !scrolled.contains("abcd"),
            "推进 4 拍后应从第 5 字符起、开头滚出: {scrolled}"
        );
        Ok(())
    }

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
