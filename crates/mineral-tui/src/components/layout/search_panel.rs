//! Search 布局态面板渲染:token prompt 输入行 + 结果列。

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use mineral_model::{ArtistRef, SearchKind};
use mineral_task::SearchPayload;

use crate::render::theme::Theme;
use crate::runtime::state::{ChannelSearchState, SearchFocus};

/// 面板边框样式:焦点态 accent 高亮,否则 overlay 暗调(spec §1.2 当前焦点面板边框高亮)。
fn border_style(focused: bool, theme: &Theme) -> Style {
    let color = if focused { theme.accent } else { theme.overlay };
    Style::new().fg(color)
}

/// 画 token prompt 输入行:`[源徽章] [类型徽章] query█`。
///
/// 源徽章颜色经 [`Theme::source_color`] 从 `SourceKind.palette()` 落地(不 match 来源,
/// 插件源自动正确)。无可搜索源(`current()` 为 `None`)画空态提示。
///
/// # Params:
///   - `rs`: channel 搜索子域(读当前源 / 会话 query / kind)
///   - `border_focused`: 边框是否高亮(焦点环滑动期由调用方置 `false`,改由浮动环表达高亮)
pub fn draw_prompt(
    frame: &mut Frame<'_>,
    area: Rect,
    rs: &ChannelSearchState,
    theme: &Theme,
    border_focused: bool,
) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(border_style(border_focused, theme))
        .title("search");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(session) = rs.current() else {
        let hint = Span::styled("no searchable source", Style::new().fg(theme.overlay));
        frame.render_widget(Paragraph::new(Line::from(hint)), inner);
        return;
    };
    let mut spans = Vec::<Span<'_>>::new();
    if let Some(source) = rs.source {
        spans.push(Span::styled(
            format!(" {} ", source.label()),
            Style::new()
                .fg(theme.source_color(source.palette()))
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(
        format!(" {} ", kind_label(session.kind)),
        Style::new().fg(theme.subtext),
    ));
    spans.push(Span::raw(" "));
    spans.push(Span::styled(
        session.query.clone(),
        Style::new().fg(theme.text),
    ));
    spans.push(Span::styled(
        "█",
        Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

/// 画结果列:bordered `results` 外框 + 结果行(当前光标行高亮)。
///
/// 光标行高亮分两档:焦点在结果列时 accent 亮高亮;否则(在 prompt / detail)走暗调高亮,
/// 仍标出"回得去"的光标位置而不抢视觉。
///
/// # Params:
///   - `rs`: channel 搜索子域(读当前会话结果与光标)
///   - `border_focused`: 边框是否高亮(焦点环滑动期由调用方置 `false`)
pub fn draw_results(
    frame: &mut Frame<'_>,
    area: Rect,
    rs: &ChannelSearchState,
    theme: &Theme,
    border_focused: bool,
) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(border_style(border_focused, theme))
        .title("results");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(session) = rs.current() else {
        return;
    };
    // 尚未搜索 / 空页:画居中 lite 提示,而非占用首行的可高亮列表项。
    if session.result_len() == 0 {
        draw_centered_hint(frame, inner, "type a query", theme);
        return;
    }
    let Some(payload) = session.results.as_ref() else {
        return;
    };
    let (header, rows, widths) = result_table(payload, theme);
    let mut table_state =
        TableState::default().with_selected(Some(session.sel.min(rows.len().saturating_sub(1))));
    // 整行底色高亮(对齐 tracks/playlist/queue 的 row_highlight):bg 铺满整行,非仅文字变色。
    // 焦点在结果列 → accent 亮;否则暗调(surface0 底 + subtext 字,无 BOLD),示意可回位。
    let highlight = if rs.focus == SearchFocus::Results {
        Style::new()
            .bg(theme.surface0)
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().bg(theme.surface0).fg(theme.subtext)
    };
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD)))
        .row_highlight_style(highlight)
        .highlight_symbol("▌ ");
    frame.render_stateful_widget(table, inner, &mut table_state);
}

/// 画详情面板外框(`detail` 标题,焦点高亮)。实体详情内容(头图 / 字段)留待后续里程碑,
/// 此处先落焦点态边框与标题,使面板切换有可见目标。
///
/// # Params:
///   - `border_focused`: 边框是否高亮(焦点环滑动期由调用方置 `false`)
pub fn draw_detail(frame: &mut Frame<'_>, area: Rect, theme: &Theme, border_focused: bool) {
    frame.render_widget(
        Block::new()
            .borders(Borders::ALL)
            .border_style(border_style(border_focused, theme))
            .title("detail"),
        area,
    );
}

/// 空结果列的居中 lite 提示(暗调斜体,水平 + 垂直居中);非可高亮列表行。
fn draw_centered_hint(frame: &mut Frame<'_>, inner: Rect, text: &str, theme: &Theme) {
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let strip = Rect::new(inner.x, inner.y + inner.height / 2, inner.width, 1);
    let hint = Line::from(text.to_owned()).style(
        Style::new()
            .fg(theme.overlay)
            .add_modifier(Modifier::ITALIC),
    );
    frame.render_widget(Paragraph::new(hint).alignment(Alignment::Center), strip);
}

/// 把一页结果载荷按类型转成「表头 + 列对齐表格行 + 列宽约束」(调用方已保证非空)。
///
/// 每类型一套列与表头:主名 Fill + 类型特有的次/计量列。一个 payload 只含单一实体类型,故按
/// 类型选一套;主名走 `text`、次列 `subtext`、计量列 `overlay`,层级与 library 表一致。计量
/// 列为裸数字,含义由表头说明(同 library 约定),省去逐行重复单位词。
fn result_table(
    payload: &SearchPayload,
    theme: &Theme,
) -> (Vec<Cell<'static>>, Vec<Row<'static>>, Vec<Constraint>) {
    let main = Style::new().fg(theme.text);
    let sub = Style::new().fg(theme.subtext);
    let meta = Style::new().fg(theme.overlay);
    match payload {
        // 歌曲:标题 · 艺人 · 时长。
        SearchPayload::Songs(songs) => {
            let rows = songs
                .iter()
                .map(|s| {
                    Row::new(vec![
                        Cell::from(Span::styled(s.name.clone(), main)),
                        Cell::from(Span::styled(join_artists(&s.artists), sub)),
                        Cell::from(Span::styled(format_duration(s.duration_ms), meta)),
                    ])
                })
                .collect();
            (
                vec![Cell::from("title"), Cell::from("artist"), Cell::from("len")],
                rows,
                vec![
                    Constraint::Fill(3),
                    Constraint::Fill(2),
                    Constraint::Length(5),
                ],
            )
        }
        // 专辑:专辑名 · 艺人。
        SearchPayload::Albums(albums) => {
            let rows = albums
                .iter()
                .map(|a| {
                    Row::new(vec![
                        Cell::from(Span::styled(a.name.clone(), main)),
                        Cell::from(Span::styled(join_artists(&a.artists), sub)),
                    ])
                })
                .collect();
            (
                vec![Cell::from("album"), Cell::from("artist")],
                rows,
                vec![Constraint::Fill(3), Constraint::Fill(2)],
            )
        }
        // 歌单:歌单名 · 曲目数(裸数字,表头标 tracks)。
        SearchPayload::Playlists(playlists) => {
            let rows = playlists
                .iter()
                .map(|p| {
                    Row::new(vec![
                        Cell::from(Span::styled(p.name.clone(), main)),
                        Cell::from(Span::styled(p.track_count.to_string(), meta)),
                    ])
                })
                .collect();
            (
                vec![Cell::from("playlist"), Cell::from("tracks")],
                rows,
                vec![Constraint::Fill(1), Constraint::Length(6)],
            )
        }
        // 歌手:歌手名 · 关注数(裸缩写,表头标 fans)。
        SearchPayload::Artists(artists) => {
            let rows = artists
                .iter()
                .map(|a| {
                    Row::new(vec![
                        Cell::from(Span::styled(a.name.clone(), main)),
                        Cell::from(Span::styled(humanize_count(a.follower_count), meta)),
                    ])
                })
                .collect();
            (
                vec![Cell::from("artist"), Cell::from("fans")],
                rows,
                vec![Constraint::Fill(1), Constraint::Length(6)],
            )
        }
    }
}

/// 多艺人名 join 成 `艺人1, 艺人2`(无艺人为空串)。
fn join_artists(artists: &[ArtistRef]) -> String {
    artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<&str>>()
        .join(", ")
}

/// 时长 ms → `m:ss`(结果列右侧;与 library 同款格式)。
fn format_duration(ms: u64) -> String {
    let secs = ms / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// 大计数缩写:< 1 万原样,≥ 1 万记 `Nk`,≥ 100 万记 `NM`(关注数列窄,纯整数无浮点)。
fn humanize_count(n: u64) -> String {
    match n {
        0..=9_999 => n.to_string(),
        10_000..=999_999 => format!("{}k", n / 1000),
        _ => format!("{}M", n / 1_000_000),
    }
}

/// 搜索类型的英文短标(token prompt 类型徽章用)。
fn kind_label(kind: SearchKind) -> &'static str {
    match kind {
        SearchKind::Song => "songs",
        SearchKind::Album => "albums",
        SearchKind::Artist => "artists",
        SearchKind::Playlist => "playlists",
        SearchKind::User => "users",
    }
}
