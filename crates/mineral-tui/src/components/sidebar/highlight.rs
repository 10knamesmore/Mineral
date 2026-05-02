//! 列表行内的搜索命中高亮:case-insensitive 切片成 Span 序列。
//!
//! 给 sidebar 两栏(playlists / library)共用,避免高亮规则在两处漂移。

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::theme::Theme;

/// case-insensitive 匹配 `q` 出现的所有位置,把命中片段切到独立 Span 上做高亮。
///
/// `base` 是非命中片段的样式;命中片段在 `base` 基础上覆盖 peach + bold + underlined。
///
/// 边界情况:`text.to_lowercase()` 改字节长度时(土耳其 İ → "i\u{307}"),
/// 直接放弃高亮 —— 字节偏移对不上原文,硬切会越界 / 切到非字符边界。
pub fn highlight<'a>(text: &str, q: &str, base: Style, theme: &Theme) -> Vec<Span<'a>> {
    if q.is_empty() {
        return vec![Span::styled(text.to_owned(), base)];
    }
    let hay = text.to_lowercase();
    let needle = q.to_lowercase();
    if hay.len() != text.len() || needle.is_empty() {
        return vec![Span::styled(text.to_owned(), base)];
    }
    let hit = base
        .fg(theme.peach)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let mut out = Vec::<Span<'a>>::new();
    let mut cursor = 0usize;
    while let Some(rel) = hay.get(cursor..).and_then(|s| s.find(&needle)) {
        let abs = cursor.saturating_add(rel);
        let end = abs.saturating_add(needle.len());
        if let Some(pre) = text.get(cursor..abs)
            && !pre.is_empty()
        {
            out.push(Span::styled(pre.to_owned(), base));
        }
        let Some(hit_str) = text.get(abs..end) else {
            break;
        };
        out.push(Span::styled(hit_str.to_owned(), hit));
        cursor = end;
    }
    if let Some(rest) = text.get(cursor..)
        && !rest.is_empty()
    {
        out.push(Span::styled(rest.to_owned(), base));
    }
    out
}
