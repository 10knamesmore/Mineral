//! 列表行内的搜索命中高亮:按 char 下标切片成 Span 序列。
//!
//! 给 sidebar 两栏(playlists / library)共用,避免高亮规则在两处漂移。
//!
//! 入参 `hits` 是已 sort + dedup 的 `text` 字符下标(单位 char,非 byte),
//! 由 `filter::FuzzyMatcher` 反向映射出 —— 既覆盖原文段直接命中,也覆盖
//! 拼音 / 首字母命中后映射回的汉字位置。

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::render::theme::Theme;

/// 按 char 下标把 `text` 切片成 base / hit Span 序列。
///
/// `hits` 为空 → 整段 `base` 样式。
/// 命中段:`base` + peach + bold + underlined。
///
/// 连续 hit 合并为单段;非 hit 段同样合并 —— 减少 ratatui Span 数量,渲染更快。
pub fn highlight_indices<'a>(
    text: &str,
    hits: &[u32],
    base: Style,
    theme: &Theme,
) -> Vec<Span<'a>> {
    if hits.is_empty() {
        return vec![Span::styled(text.to_owned(), base)];
    }
    let hit_style = base
        .fg(theme.peach)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);

    let mut out = Vec::<Span<'a>>::new();
    let mut buf = String::new();
    let mut in_hit = false;
    let mut hit_iter = hits.iter().copied().peekable();

    for (idx, ch) in text.chars().enumerate() {
        let Ok(idx_u32) = u32::try_from(idx) else {
            // text > 2^32 chars 不可能;万一出现,后续 char 一律按 base 走。
            buf.push(ch);
            continue;
        };
        // 滚动 hit_iter 跳过已经过去的下标(防御:hits 是 sorted dedup'd 的)。
        while hit_iter.peek().is_some_and(|&h| h < idx_u32) {
            hit_iter.next();
        }
        let is_hit = hit_iter.peek().copied() == Some(idx_u32);
        if is_hit != in_hit {
            if !buf.is_empty() {
                let style = if in_hit { hit_style } else { base };
                out.push(Span::styled(std::mem::take(&mut buf), style));
            }
            in_hit = is_hit;
        }
        buf.push(ch);
    }
    if !buf.is_empty() {
        let style = if in_hit { hit_style } else { base };
        out.push(Span::styled(buf, style));
    }
    out
}

#[cfg(test)]
mod tests {
    use ratatui::style::Style;

    use super::highlight_indices;
    use crate::render::theme::Theme;

    /// 拿 spans 的 `content` 字符串(转成 `&str`)序列,便于 `assert_eq!` 整体比较。
    fn contents<'a>(spans: &'a [ratatui::text::Span<'a>]) -> Vec<&'a str> {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// 空 hits → 整段 base 样式,单 Span。
    #[test]
    fn empty_hits_single_base_span() {
        let theme = Theme::default();
        let spans = highlight_indices("春日影", &[], Style::new(), &theme);
        assert_eq!(contents(&spans), vec!["春日影"]);
    }

    /// 全命中 → 单 hit Span。
    #[test]
    fn all_hits_single_hit_span() {
        let theme = Theme::default();
        let spans = highlight_indices("春日影", &[0, 1, 2], Style::new(), &theme);
        assert_eq!(contents(&spans), vec!["春日影"]);
    }

    /// 间断命中:`春_影` 高亮 → ["春" hit, "日" base, "影" hit] 三段。
    #[test]
    fn scattered_hits_splits_into_runs() {
        let theme = Theme::default();
        let spans = highlight_indices("春日影", &[0, 2], Style::new(), &theme);
        assert_eq!(contents(&spans), vec!["春", "日", "影"]);
    }

    /// 混 ASCII + Han:hits 落在 ASCII 段。
    #[test]
    fn ascii_hits_in_mixed() {
        let theme = Theme::default();
        let spans = highlight_indices("春 MyGO", &[2, 3, 4, 5], Style::new(), &theme);
        assert_eq!(contents(&spans), vec!["春 ", "MyGO"]);
    }

    /// 越界 hit 不导致 panic / index error,直接被丢弃。
    #[test]
    fn out_of_range_hits_skipped() {
        let theme = Theme::default();
        let spans = highlight_indices("ab", &[0, 5], Style::new(), &theme);
        assert_eq!(contents(&spans), vec!["a", "b"]);
    }
}
