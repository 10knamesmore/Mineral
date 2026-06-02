//! Playlists 视图渲染。

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Paragraph, Row, StatefulWidget, Table, TableState, Widget,
};

use super::badge::search_badge;
use super::highlight::highlight;
use crate::render::theme::Theme;
use crate::runtime::state::AppState;
use crate::runtime::view_model::PlaylistView;

/// 渲染 Playlists 视图到给定 [`Buffer`](正常渲染与离屏过渡合成共用此入口)。
pub fn render_to(buf: &mut Buffer, area: Rect, state: &AppState, theme: &Theme) {
    let rows_data = state.filtered_playlists();
    let total = rows_data.len();
    let pos = position_label(state.sel_playlist, total);

    let mut title_spans = vec![Span::styled(" playlists ", Style::new().fg(theme.subtext))];
    title_spans.extend(search_badge(state, theme));

    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(title_spans))
        .title_bottom(Line::from(pos).style(Style::new().fg(theme.overlay)));

    // 全空 + 无搜索词:走 empty-state 提示分支(loading / 未登录二选一)。
    // 区分依据是 tasks_running:有任务在跑就是 loading,没任务就大概率是
    // 没登录任何源 / 各源都无歌单 —— 给出登录引导。
    if state.playlists.is_empty() && state.search_q.is_empty() {
        paint_empty_state(buf, area, state, theme, block);
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
    StatefulWidget::render(table, area, buf, &mut table_state);
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
    let src = p.data.source();

    Row::new(vec![
        Cell::from(Line::from(highlight(
            &p.data.name,
            &state.search_q,
            Style::new().fg(theme.text),
            theme,
        ))),
        Cell::from(Span::styled(
            src.label(),
            Style::new().fg(theme.source_color(src.palette())),
        )),
        Cell::from(Span::styled(len_label, Style::new().fg(theme.subtext))),
        Cell::from(Span::styled(count_label, Style::new().fg(theme.overlay))),
    ])
}

/// 全空 playlist 时画 block + 居中两行提示。loading / 未登录文案二选一。
fn paint_empty_state(
    buf: &mut Buffer,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    block: Block<'_>,
) {
    let inner = block.inner(area);
    block.render(area, buf);
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
        // 空 = 所有源都没产出(未登录 / 该源无歌单)。不写死单源:列一个登录示例,
        // `例如` 体现多源可扩展;命令是完整子命令链(bin=mineral)。
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "还没有可用歌单",
                Style::new().fg(theme.subtext),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "登录一个音乐源,例如:",
                Style::new().fg(theme.overlay),
            )),
            Line::from(Span::styled(
                "  mineral channel netease login",
                Style::new().fg(theme.peach).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "登录后重启 mineral 生效",
                Style::new().fg(theme.overlay),
            )),
        ]
    };
    Paragraph::new(lines)
        .alignment(ratatui::layout::Alignment::Center)
        .render(inner, buf);
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
    use crate::render::theme::Theme;
    use crate::runtime::state::AppState;

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
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "歌单列表:3 个混源歌单(EndSerenading / 本地)",
            t.backend()
        );
        Ok(())
    }

    /// 搜索输入态:标题挂 `/查询█`(末尾光标方块,表示正在输入)。
    #[test]
    fn playlists_search_active_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        let mut state = crate::test_support::state_with_playlists();
        state.search_mode = true;
        state.search_q = "春日影".to_owned();
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!("歌单列表:搜索输入态(标题 /春日影█)", t.backend());
        Ok(())
    }

    /// 空列表(尚未加载 / 未登录)。
    #[test]
    fn playlists_empty_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        let state = AppState::empty();
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!("歌单列表:空态(未加载 / 未登录)", t.backend());
        Ok(())
    }
}
