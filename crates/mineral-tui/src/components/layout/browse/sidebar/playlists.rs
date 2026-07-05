//! Playlists 视图渲染。

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table, Widget};

use super::badge::search_badge;
use super::highlight::highlight_indices;
use crate::components::layout::shared::scroll_table::render_scroll_table;
use crate::render::theme::Theme;
use crate::runtime::scroll::list::ScrollMotion;
use crate::runtime::state::AppState;
use crate::runtime::view_model::PlaylistView;

/// 渲染 Playlists 视图到给定 [`Buffer`](正常渲染与离屏过渡合成共用此入口)。
pub fn render_to(buf: &mut Buffer, area: Rect, state: &AppState, theme: &Theme) {
    let rows_data = state.filtered_playlists();
    let total = rows_data.len();
    let pos = position_label(state.browse.nav.playlist.sel(), total);

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
    if state.library.playlists.is_empty() && state.browse.search.query().is_empty() {
        paint_empty_state(buf, area, state, theme, block);
        return;
    }

    // 有词但零命中:给居中提示而非纯空白。深度索引还在飞时说「索引中」——
    // 此刻搜不到 ≠ 真没有,数据到齐后结果可能变。
    if total == 0 && !state.browse.search.query().is_empty() {
        paint_no_match(buf, area, state, theme, block);
        return;
    }

    // 确有深度命中时多一列「match」展示歌单内命中歌曲;纯歌单名命中 / 空 query
    // 不占位,不挤压 name 列宽。
    let show_match = state.has_deep_hits();
    let mut header_cells = vec![Cell::from("name")];
    if show_match {
        header_cells.push(Cell::from("match"));
    }
    header_cells.extend([
        Cell::from("source"),
        Cell::from("length"),
        Cell::from("items"),
    ]);
    let header =
        Row::new(header_cells).style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));

    let rows: Vec<Row<'_>> = rows_data
        .into_iter()
        .map(|p| build_row(p, state, theme, show_match))
        .collect();

    // name 列用 Fill 取「剩余空间」而非 Min:Min 在有 slack 时会给 ratatui 列宽求解器
    // 留多解(name>=12 + 总宽等式欠定),解不唯一 → 列宽随机差 1、帧间闪烁;Fill(1)
    // 是 name = 总宽 - 其余定宽列,唯一解,确定性。
    let mut widths = vec![Constraint::Fill(1)];
    if show_match {
        widths.push(Constraint::Fill(1));
    }
    widths.extend([
        Constraint::Length(11),
        Constraint::Length(8),
        Constraint::Length(5),
    ]);

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

    // 视口行数 = 面板高 - 上下边框 - 表头;offset 跨帧持久(nvim 手感),滚动经缓动平移。
    // 全屏 morph 中面板 rect 是插值瞬态:只读展示(Frozen),理由同 library。
    let viewport = usize::from(area.height.saturating_sub(3));
    let motion = if state.browse.fullscreen.at_min() {
        ScrollMotion::Advancing {
            scrolloff: state.scrolloff(),
            glide_ticks: state.list_glide_ticks(),
        }
    } else {
        ScrollMotion::Frozen
    };
    render_scroll_table(
        buf,
        area,
        table,
        &state.browse.nav.playlist,
        total,
        viewport,
        motion,
    );
}

/// 把一个歌单组装成 sidebar 表格行(名字 [/ 深度命中] / 来源 / 总时长 / 曲目数)。
fn build_row<'a>(
    p: &'a PlaylistView,
    state: &AppState,
    theme: &Theme,
    show_match: bool,
) -> Row<'a> {
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

    let name_hits = state.browse.search.match_for(&p.data.name).map(|m| m.hits);
    let mut cells = vec![Cell::from(Line::from(highlight_indices(
        &p.data.name,
        name_hits.as_deref().unwrap_or(&[]),
        Style::new().fg(theme.text),
        theme,
    )))];
    if show_match {
        cells.push(deep_hit_cell(p, state, theme));
    }
    cells.extend([
        Cell::from(Span::styled(
            src.label(),
            Style::new().fg(crate::render::theme::resolve_source_color(
                theme,
                state.cfg.sources(),
                src,
            )),
        )),
        Cell::from(Span::styled(len_label, Style::new().fg(theme.subtext))),
        Cell::from(Span::styled(count_label, Style::new().fg(theme.overlay))),
    ]);
    Row::new(cells)
}

/// 深度命中列:`♪ 歌名 · 艺人/专辑`(命中字符同款高亮)+ `+n` 计数;
/// 该歌单无歌曲命中时为空白格。
fn deep_hit_cell<'a>(p: &PlaylistView, state: &AppState, theme: &Theme) -> Cell<'a> {
    let Some(hit) = state.deep_hit_for(&p.data.id) else {
        return Cell::from("");
    };
    let mut spans = highlight_indices(&hit.line, &hit.hits, Style::new().fg(theme.subtext), theme);
    if hit.extra > 0 {
        spans.push(Span::styled(
            format!(" +{}", hit.extra),
            Style::new().fg(theme.overlay),
        ));
    }
    Cell::from(Line::from(spans))
}

/// 搜索零命中时画 block + 居中提示;深度索引在飞时改提示「索引中」。
fn paint_no_match(buf: &mut Buffer, area: Rect, state: &AppState, theme: &Theme, block: Block<'_>) {
    let inner = block.inner(area);
    block.render(area, buf);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let text = if let Some(n) = super::badge::indexing_count(state) {
        format!("索引中({n} 个歌单)…")
    } else {
        "无匹配".to_owned()
    };
    Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(text, Style::new().fg(theme.overlay))),
    ])
    .alignment(ratatui::layout::Alignment::Center)
    .render(inner, buf);
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
        let state = crate::test_support::state_with_playlists()?;
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
        let mut state = crate::test_support::state_with_playlists()?;
        state.browse.search.typing = true;
        state.browse.search.set_query("春日影");
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!("歌单列表:搜索输入态(标题 /春日影█)", t.backend());
        Ok(())
    }

    /// 拼音首字母搜索:输入 `cry` → 命中「春日影」,Han 三字均高亮(反向映射)。
    #[test]
    fn playlists_search_pinyin_initials_snapshot() -> color_eyre::Result<()> {
        use mineral_model::SourceKind;

        let mut state = AppState::test_default()?;
        state.library.playlists = vec![
            crate::test_support::playlist_view("a", "MyGO!!!!!", SourceKind::NETEASE, 1),
            crate::test_support::playlist_view("b", "春日影", SourceKind::NETEASE, 1),
        ];
        state.browse.search.set_query("cry");
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "歌单列表:首字母 cry 命中「春日影」(汉字三字均高亮)",
            t.backend()
        );
        Ok(())
    }

    /// 全拼搜索:输入 `chunying` → 命中「春日影」,春 + 影 高亮(日的 ri 未命中)。
    #[test]
    fn playlists_search_pinyin_full_snapshot() -> color_eyre::Result<()> {
        use mineral_model::SourceKind;

        let mut state = AppState::test_default()?;
        state.library.playlists = vec![crate::test_support::playlist_view(
            "a",
            "春日影",
            SourceKind::NETEASE,
            1,
        )];
        state.browse.search.set_query("chunying");
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "歌单列表:全拼 chunying 命中「春日影」(春 + 影 高亮)",
            t.backend()
        );
        Ok(())
    }

    /// 深度命中:歌单名均不含搜索词,p2 内「春日影 · CRYCHIC」命中 → 该歌单被捞出,
    /// match 列展示 `♪ 春日影 · CRYCHIC`(命中字符同款高亮)。
    #[test]
    fn playlists_search_deep_hit_snapshot() -> color_eyre::Result<()> {
        use mineral_model::{PlaylistId, SourceKind};

        use crate::runtime::view_model::SongView;
        use crate::test_support::{song, with_artist, with_name};

        let mut state = crate::test_support::state_with_playlists()?;
        let track = with_artist(with_name(song("s1"), "春日影"), "CRYCHIC");
        state.library.tracks.insert(
            PlaylistId::new(SourceKind::NETEASE, "p2"),
            vec![SongView {
                data: track,
                loved: false,
                plays: None,
            }],
        );
        state.library.tracks_generation = 1;
        state.browse.search.set_query("春日");

        let mut t = Terminal::new(TestBackend::new(64, 12))?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "歌单列表:深度命中(match 列展示 ♪ 春日影 · CRYCHIC,命中高亮)",
            t.backend()
        );
        Ok(())
    }

    /// 搜索零命中(无在飞索引):居中「无匹配」提示而非纯空白。
    #[test]
    fn playlists_search_no_match_snapshot() -> color_eyre::Result<()> {
        let mut state = crate::test_support::state_with_playlists()?;
        state.browse.search.set_query("zzz");
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!("歌单列表:搜索零命中(居中「无匹配」)", t.backend());
        Ok(())
    }

    /// 搜索零命中 + 深度索引在飞:badge 缀 ⟳n,居中提示「索引中」——
    /// 此刻搜不到 ≠ 真没有。
    #[test]
    fn playlists_search_indexing_snapshot() -> color_eyre::Result<()> {
        let mut state = crate::test_support::state_with_playlists()?;
        state.browse.search.set_query("zzz");
        state
            .tasks_snapshot
            .by_kind
            .insert(mineral_task::ChannelFetchKindTag::PlaylistDetail, 3);
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!(
            "歌单列表:零命中且索引在飞(badge ⟳3 + 居中「索引中」)",
            t.backend()
        );
        Ok(())
    }

    /// 空列表(尚未加载 / 未登录)。
    #[test]
    fn playlists_empty_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        let state = AppState::test_default()?;
        t.draw(|f| {
            let area = f.area();
            super::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        crate::test_support::assert_snap!("歌单列表:空态(未加载 / 未登录)", t.backend());
        Ok(())
    }
}
