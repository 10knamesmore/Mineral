//! 搜索 badge:把当前搜索态画进左栏面板标题。

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::render::theme::Theme;
use crate::runtime::state::AppState;

/// 把搜索态渲染成可拼进面板标题的 [`Span`] 序列。
///
/// # Params:
///   - `state`: 读 `search_mode`(是否正在输入)与 `search_q`(当前词)
///   - `theme`: 取色
///
/// # Return:
///   - 输入态(`search_mode`):`/q█`(末尾光标方块,表示正在输入)
///   - 非输入态但有词:`/q`(已提交、仍在过滤的词)
///   - 无词且非输入态:空序列(标题不挂 badge)
pub fn search_badge(state: &AppState, theme: &Theme) -> Vec<Span<'static>> {
    if !state.search_mode && state.search_q.is_empty() {
        return Vec::new();
    }
    let mut spans = vec![Span::styled(
        format!("/{}", state.search_q),
        Style::new().fg(theme.peach),
    )];
    if state.search_mode {
        spans.push(Span::styled(
            "█",
            Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
        ));
    }
    spans
}
