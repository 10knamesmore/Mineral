//! Playlists 视图渲染。

use mineral_model::SourceKind;
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table, TableState};

use super::highlight::highlight;
use crate::state::AppState;
use crate::theme::Theme;
use crate::view_model::PlaylistView;

/// 渲染 Playlists 视图到给定 [`Rect`]。
pub fn draw(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let rows_data = state.filtered_playlists();
    let total = rows_data.len();
    let pos = position_label(state.sel_playlist, total);

    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(vec![
            Span::styled(" playlists ", Style::new().fg(theme.subtext)),
            search_badge(&state.search_q, theme),
        ]))
        .title_bottom(Line::from(pos).style(Style::new().fg(theme.overlay)));

    // 全空 + 无搜索词:走 empty-state 提示分支(loading / 未登录二选一)。
    // 区分依据是 tasks_running:有任务在跑就是 loading,没任务就大概率是
    // 没 cookie / 拉失败 —— 直接给出 `mineral-cli login` 引导。
    if state.playlists.is_empty() && state.search_q.is_empty() {
        paint_empty_state(frame, area, state, theme, block);
        return;
    }

    let header = Row::new(vec![
        Cell::from("name"),
        Cell::from("source"),
        Cell::from("length"),
        Cell::from("items"),
    ])
    .style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));

    let rows: Vec<Row<'_>> = rows_data
        .into_iter()
        .map(|p| build_row(p, state, theme))
        .collect();

    // name 列用 Fill 取「剩余空间」而非 Min:Min 在有 slack 时会给 ratatui 列宽求解器
    // 留多解(name>=12 + 总宽等式欠定),解不唯一 → 列宽随机差 1、帧间闪烁;Fill(1)
    // 是 name = 总宽 - 其余定宽列,唯一解,确定性。
    let widths = [
        Constraint::Fill(1),
        Constraint::Length(11),
        Constraint::Length(8),
        Constraint::Length(5),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(
            Style::new()
                .bg(theme.surface0)
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▌ ");

    let mut table_state = TableState::default();
    table_state.select(Some(state.sel_playlist));
    frame.render_stateful_widget(table, area, &mut table_state);
}

/// 把一个歌单组装成 sidebar 表格行(名字 / 来源 channel / 总时长 / 曲目数)。
fn build_row<'a>(p: &'a PlaylistView, state: &AppState, theme: &Theme) -> Row<'a> {
    let total_ms = state.total_duration_ms_of(&p.data.id);
    let len_label = if total_ms == 0 {
        String::from("—")
    } else {
        let total_min = total_ms / 60_000;
        let h = total_min / 60;
        let m = total_min % 60;
        if h == 0 {
            format!("{m}m")
        } else {
            format!("{h}h {m:02}m")
        }
    };
    let count_label = format!("{}", p.data.track_count);

    Row::new(vec![
        Cell::from(Line::from(highlight(
            &p.data.name,
            &state.search_q,
            Style::new().fg(theme.text),
            theme,
        ))),
        Cell::from(Span::styled(
            channel_label(p.data.source),
            channel_style(p.data.source, theme),
        )),
        Cell::from(Span::styled(len_label, Style::new().fg(theme.subtext))),
        Cell::from(Span::styled(count_label, Style::new().fg(theme.overlay))),
    ])
}

/// 把 [`SourceKind`] 翻成「图标 + 名字」的短标签(playlists 列用)。
fn channel_label(src: SourceKind) -> &'static str {
    match src {
        SourceKind::Netease => "♫ netease",
        SourceKind::Local => "□ local",
        #[cfg(feature = "mock")]
        SourceKind::Mock => "▒ mock",
    }
}

/// 把 [`SourceKind`] 映射到 channel 标签的染色。
fn channel_style(src: SourceKind, theme: &Theme) -> Style {
    match src {
        SourceKind::Netease => Style::new().fg(theme.red),
        SourceKind::Local => Style::new().fg(theme.subtext),
        #[cfg(feature = "mock")]
        SourceKind::Mock => Style::new().fg(theme.overlay),
    }
}

/// 搜索 badge:`/q` 形式,空 query 不渲染。
fn search_badge<'a>(q: &'a str, theme: &Theme) -> Span<'a> {
    if q.is_empty() {
        Span::raw("")
    } else {
        Span::styled(format!("/{q}"), Style::new().fg(theme.peach))
    }
}

/// 全空 playlist 时画 block + 居中两行提示。loading / 未登录文案二选一。
fn paint_empty_state(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    block: Block<'_>,
) {
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let lines: Vec<Line<'_>> = if state.tasks_snapshot.running > 0 {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "loading playlists…",
                Style::new().fg(theme.subtext),
            )),
        ]
    } else {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "尚未登录或拉取失败",
                Style::new().fg(theme.subtext),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "请先在另一个终端跑:",
                Style::new().fg(theme.overlay),
            )),
            Line::from(Span::styled(
                "  mineral-cli login",
                Style::new().fg(theme.peach).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "登录完成后重启本程序",
                Style::new().fg(theme.overlay),
            )),
        ]
    };
    frame.render_widget(
        Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center),
        inner,
    );
}

/// 拼 ` n / total ` 的 footer 标签;空列表显示 `0 / 0`。
fn position_label(sel: usize, total: usize) -> String {
    if total == 0 {
        " 0 / 0 ".to_owned()
    } else {
        format!(" {} / {total} ", sel.saturating_add(1).min(total))
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::position_label;
    use crate::state::AppState;
    use crate::theme::Theme;

    /// `position_label`:1-based 当前位 / 总数;空列表 `0 / 0`;越界 clamp。
    #[test]
    fn position_label_cases() {
        assert_eq!(position_label(0, 0), " 0 / 0 ");
        assert_eq!(position_label(0, 3), " 1 / 3 ");
        assert_eq!(position_label(2, 3), " 3 / 3 ");
        assert_eq!(position_label(9, 3), " 3 / 3 ");
    }

    /// 3 个混源歌单列表(name 列 Fill(1) 后列宽确定,不再 flaky)。
    #[test]
    fn playlists_list_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        let state = crate::test_support::state_with_playlists();
        t.draw(|f| super::draw(f, f.area(), &state, &Theme::default()))?;
        crate::test_support::assert_snap!(
            "歌单列表:3 个混源歌单(EndSerenading / 本地)",
            t.backend()
        );
        Ok(())
    }

    /// 空列表(尚未加载 / 未登录)。
    #[test]
    fn playlists_empty_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        let state = AppState::empty();
        t.draw(|f| super::draw(f, f.area(), &state, &Theme::default()))?;
        crate::test_support::assert_snap!("歌单列表:空态(未加载 / 未登录)", t.backend());
        Ok(())
    }
}
