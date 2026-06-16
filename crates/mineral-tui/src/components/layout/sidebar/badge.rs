//! 搜索 badge:把当前搜索态画进左栏面板标题。

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::render::theme::Theme;
use crate::runtime::state::{AppState, View};

/// 把搜索态渲染成可拼进面板标题的 [`Span`] 序列。
///
/// # Params:
///   - `state`: 读 `search.typing`(是否正在输入)与 `search.query`(当前词)
///   - `theme`: 取色
///
/// # Return:
///   - 输入态(`search.typing`):`/q█`(光标块在文本光标处,表示正在输入)
///   - 非输入态但有词:`/q`(已提交、仍在过滤的词)
///   - 任一搜索态下深度索引在飞:再缀 ` ⟳n`(见 [`indexing_count`])
///   - 无词且非输入态:空序列(标题不挂 badge)
pub fn search_badge(state: &AppState, theme: &Theme) -> Vec<Span<'static>> {
    if !state.search.typing && state.search.query().is_empty() {
        return Vec::new();
    }
    let mut spans = Vec::<Span<'static>>::new();
    if state.search.typing {
        // 输入态:光标块落在文本光标处(before|after),不再恒在词尾。
        let (before, after) = state.search.query_split();
        spans.push(Span::styled(
            format!("/{before}"),
            Style::new().fg(theme.peach),
        ));
        spans.push(Span::styled(
            "█",
            Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(after.to_owned(), Style::new().fg(theme.peach)));
    } else {
        // 非输入态:已提交、仍在过滤的词,无光标。
        spans.push(Span::styled(
            format!("/{}", state.search.query()),
            Style::new().fg(theme.peach),
        ));
    }
    if let Some(n) = indexing_count(state) {
        spans.push(Span::styled(
            format!(" ⟳{n}"),
            Style::new().fg(theme.overlay),
        ));
    }
    spans
}

/// 深度索引在飞的歌单数:Playlists 视图 + deep 开启 + PlaylistDetail 任务计数 > 0。
/// 不在此状态返回 `None`(badge 不缀)。搜不到时用户据此区分「真没有」和「还没拉完」。
pub fn indexing_count(state: &AppState) -> Option<usize> {
    if state.view != View::Playlists || !*state.cfg.tui().search().deep() {
        return None;
    }
    let n = state
        .tasks_snapshot
        .by_kind
        .get(&mineral_task::ChannelFetchKindTag::PlaylistDetail)
        .copied()
        .unwrap_or(0);
    (n > 0).then_some(n)
}
