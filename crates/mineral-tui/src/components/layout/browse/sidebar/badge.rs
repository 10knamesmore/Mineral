//! 搜索 badge:把指定面板的搜索态画进左栏标题。

use ratatui::style::Style;
use ratatui::text::Span;

use crate::render::cursor::cursor_spans;
use crate::render::theme::Theme;
use crate::runtime::state::{AppState, SearchState};

/// 把指定面板的搜索态渲染成可拼进标题的 [`Span`] 序列。
///
/// # Params:
///   - `search`: 正在绘制的面板搜索态，过渡期间不跟随逻辑当前视图
///   - `indexing`: 该面板需要显示的深度索引数量；Library 传 `None`
///   - `theme`: 取色
///
/// # Return:
///   - 输入态:`/q`+反色光标罩在文本光标处字符上
///   - 非输入态但有词:`/q`
///   - 传入索引数量时再缀 ` ⟳n`
///   - 无词且非输入态:空序列
pub fn search_badge(
    search: &SearchState,
    indexing: Option<usize>,
    theme: &Theme,
) -> Vec<Span<'static>> {
    if !search.typing && search.query().is_empty() {
        return Vec::new();
    }
    let mut spans = Vec::<Span<'static>>::new();
    if search.typing {
        // 输入态:光标反色罩在 `before|after` 分隔处的字符上。
        let (before, after) = search.query_split();
        let base = Style::new().fg(theme.peach);
        spans.extend(cursor_spans(format!("/{before}"), after, base));
    } else {
        spans.push(Span::styled(
            format!("/{}", search.query()),
            Style::new().fg(theme.peach),
        ));
    }
    if let Some(n) = indexing {
        spans.push(Span::styled(
            format!(" ⟳{n}"),
            Style::new().fg(theme.overlay),
        ));
    }
    spans
}

/// 深度索引中正在补齐的歌单数；首批预览不重复计数。
/// 深度搜索关闭或没有待补齐歌单时返回 `None`。
pub fn indexing_count(state: &AppState) -> Option<usize> {
    if !*state.cfg.tui().search().deep().enabled() {
        return None;
    }
    let n = state.library.completing_playlists();
    (n > 0).then_some(n)
}
