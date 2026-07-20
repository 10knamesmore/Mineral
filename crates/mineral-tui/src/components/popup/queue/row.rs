//! queue 表格的行组装。

use mineral_model::Song;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Row};
use smallvec::SmallVec;

use super::columns::{QueueCols, QueueColumns};
use crate::components::layout::shared::highlight::{alias_suffix, highlight_indices};
use crate::components::layout::shared::marquee::RowMarquee;
use crate::render::theme::Theme;
use crate::runtime::format::format_ms_opt;

/// 一行各文本列的模糊命中下标(`/` 过滤态下按这些下标染 `search_hit` 色)。
///
/// 均为已 sort + dedup 的**原文 char 下标**(由 [`FuzzyMatcher`](crate::runtime::filter::FuzzyMatcher)
/// 反向映射)。无过滤词时三列皆空,`highlight_indices` 退化为整段原样——渲染与无搜索一致。
#[derive(Default)]
pub(super) struct RowHits {
    /// 歌名命中。
    pub(super) name: SmallVec<[u32; 8]>,

    /// 别名(译名 / 副标题)命中。
    pub(super) alias: SmallVec<[u32; 8]>,

    /// 展示艺人(首位)命中。
    pub(super) artist: SmallVec<[u32; 8]>,

    /// 专辑名命中。
    pub(super) album: SmallVec<[u32; 8]>,
}

/// 一行的可变装饰:在播标记、收藏态、源色、跑马灯、模糊命中。
///
/// 收成一份随行传递,避免行组装函数摊开五六个位置参数。
pub(super) struct RowDecor<'m> {
    /// 该行是否是当前在播曲。
    pub(super) is_current: bool,

    /// 该行是否已收藏。
    pub(super) loved: bool,

    /// 序号列的源色(整列同色即该队列单一来源)。
    pub(super) index_fg: ratatui::style::Color,

    /// 选中行的跑马灯上下文;非选中行为 `None`。
    pub(super) marquee: Option<RowMarquee<'m>>,

    /// 各文本列的模糊命中下标(无过滤词时全空)。
    pub(super) hits: RowHits,
}

/// 把一首歌组成 queue 表格的一行。
///
/// 当前在播行:行首 `▶` + 整行 `accent` 前景(与选中行的「背景块」区分),在播标记
/// 语义优先于源色;其余行序号用源色,歌名用主文本色,艺术家用次要色,层级分明。
/// 选中行的高亮背景由 Table 的 `row_highlight_style` 叠加,与在播前景着色天然兼容
/// (背景块视觉优先)。`cols` 决定列集:窄档省去 artist,宽档多出 album。
///
/// # Params:
///   - `idx`: 该行在队列中的下标
///   - `song`: 该行歌曲
///   - `theme`: 主题色板
///   - `cols`: 列规格
///   - `decor`: 该行的可变装饰
///
/// # Return:
///   组装好的表格行。
pub(super) fn build_row<'a>(
    idx: usize,
    song: &'a Song,
    theme: &Theme,
    cols: QueueColumns,
    decor: RowDecor<'_>,
) -> Row<'a> {
    let (lead, title_fg, sub_fg) = if decor.is_current {
        (
            Span::styled("▶", Style::new().fg(theme.accent)),
            theme.accent,
            theme.accent,
        )
    } else {
        (
            Span::styled(idx.to_string(), Style::new().fg(decor.index_fg)),
            theme.text,
            theme.subtext,
        )
    };

    let mut title_spans = highlight_indices(
        &song.name,
        &decor.hits.name,
        Style::new().fg(title_fg),
        theme,
    );
    if let Some(alias) = song.alias.as_deref() {
        // 过滤命中别名时高亮命中字符;无过滤词(hits 空)退化为整段暗调,与非搜索一致。
        title_spans.extend(alias_suffix(alias, &decor.hits.alias, theme));
    }
    let title_cell = match decor.marquee {
        Some(m) => Cell::from(
            m.ctx
                .line(title_spans, m.slot, &song.id.qualified(), m.title_w),
        ),
        None => Cell::from(Line::from(title_spans)),
    };
    let mut cells = vec![love_cell(decor.loved, theme), Cell::from(lead), title_cell];
    if matches!(cols.text, QueueCols::Wide | QueueCols::Full) {
        let artist = song
            .artists
            .first()
            .map_or_else(|| "—".to_owned(), |a| a.name.clone());
        cells.push(Cell::from(Line::from(highlight_indices(
            &artist,
            &decor.hits.artist,
            Style::new().fg(sub_fg),
            theme,
        ))));
    }
    if matches!(cols.text, QueueCols::Wide) {
        let album = song
            .album
            .as_ref()
            .map_or_else(|| "—".to_owned(), |a| a.name.clone());
        cells.push(Cell::from(Line::from(highlight_indices(
            &album,
            &decor.hits.album,
            Style::new().fg(sub_fg),
            theme,
        ))));
    }
    cells.push(Cell::from(Span::styled(
        format_ms_opt(song.duration_ms),
        Style::new().fg(sub_fg),
    )));
    let row = Row::new(cells);
    if decor.is_current {
        // 在播行下缘加一条下划线,把「已播」与「待播」分开。用修饰而非插一行分隔:
        // 插行会让表格行下标与队列下标错位,选中高亮 / 滚动 offset / 菜单锚点三处都要
        // 跟着做映射,平白多出一片 off-by-one 面;下划线由终端画在字符底边,不占行。
        row.style(Style::new().add_modifier(ratatui::style::Modifier::UNDERLINED))
    } else {
        row
    }
}

/// 收藏 gutter:已收藏画实心 ♥(红),未收藏留空(恒占一格,像 vim signcolumn)。
///
/// 未收藏不画空心 ♡ —— 队列多数行未收藏,满屏空心会把这列变成噪音,反而盖过真正
/// 需要一眼看见的实心标记。与曲目表 `love_cell` 同款。
fn love_cell(loved: bool, theme: &Theme) -> Cell<'static> {
    if loved {
        Cell::from(Span::styled("♥", Style::new().fg(theme.red)))
    } else {
        Cell::from("")
    }
}
