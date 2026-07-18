//! 列表行内的搜索命中高亮:按 char 下标切片成 Span 序列。
//!
//! 给 sidebar 两栏(playlists / library)共用,避免高亮规则在两处漂移。
//!
//! 入参 `hits` 是已 sort + dedup 的 `text` 字符下标(单位 char,非 byte),
//! 由 `filter::FuzzyMatcher` 反向映射出 —— 既覆盖原文段直接命中,也覆盖
//! 拼音 / 首字母命中后映射回的汉字位置。

use ratatui::style::Style;
use ratatui::text::Span;

use crate::render::theme::Theme;

/// 主字段(歌名 / 艺人 / 专辑 / 歌单名)的命中高亮:命中段换成主题的 `search_hit`
/// 前景色并叠字体效果(Lua `tui.theme.search_hit` 可配)。
///
/// `hits` 为空 → 整段 `base` 样式。连续同类段合并 —— 减少 ratatui Span 数量,渲染更快。
pub fn highlight_indices<'a>(
    text: &str,
    hits: &[u32],
    base: Style,
    theme: &Theme,
) -> Vec<Span<'a>> {
    let hit_style = base
        .fg(theme.search_hit_color)
        .add_modifier(theme.search_hit_modifier);
    slice_by_hits(text, hits, base, hit_style)
}

/// 别名(译名 / 副标题)的括注后缀 ` (alias)`:括号与未命中字符 overlay 暗调
/// (与无搜索词时的浏览样式一致),命中字符换 `search_hit` 色 + 字体效果——与主字段
/// 命中同款,不因从属地位弱化。
///
/// library 曲目行的歌名后缀与 playlists 深度命中列共用,别名样式只此一处。
pub fn alias_suffix<'a>(alias: &str, hits: &[u32], theme: &Theme) -> Vec<Span<'a>> {
    let dim = Style::new().fg(theme.overlay);
    let mut out = vec![Span::styled(" (".to_owned(), dim)];
    out.extend(highlight_indices(alias, hits, dim, theme));
    out.push(Span::styled(")".to_owned(), dim));
    out
}

/// 按 char 下标把 `text` 切成「命中段用 `hit_style` / 其余用 `base`」的 Span 序列。
///
/// `hits` 是已 sort + dedup 的 `text` 字符下标(单位 char,非 byte),由
/// `filter::FuzzyMatcher` 反向映射出——既覆盖原文段直接命中,也覆盖拼音 / 首字母命中
/// 后映射回的汉字位置。`hits` 为空 → 整段 `base`;连续同类段合并。
fn slice_by_hits<'a>(text: &str, hits: &[u32], base: Style, hit_style: Style) -> Vec<Span<'a>> {
    if hits.is_empty() {
        return vec![Span::styled(text.to_owned(), base)];
    }
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

    use super::{alias_suffix, highlight_indices};
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

    /// 主字段命中:命中段换成 search_hit 色 + 叠字体效果;非命中段保持 base 色。
    #[test]
    fn primary_hit_swaps_to_search_hit_color() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let base = Style::new().fg(theme.overlay);
        let spans = highlight_indices("春日影", &[0], base, &theme);
        let hit = spans
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有命中段"))?;
        assert_eq!(
            hit.style.fg,
            Some(theme.search_hit_color),
            "主字段命中换 search_hit 色"
        );
        assert!(
            hit.style.add_modifier.contains(theme.search_hit_modifier),
            "命中段叠 search_hit 字体效果"
        );
        Ok(())
    }

    /// 别名括注:括号与未命中字符 overlay 暗调,命中字符换 search_hit 色 + 字体效果
    /// (与主字段命中同款,不因从属地位弱化)。
    #[test]
    fn alias_suffix_dim_wrapper_primary_hits() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let spans = alias_suffix("Mayoiuta", &[0, 1, 2, 3], &theme);
        assert_eq!(contents(&spans), vec![" (", "Mayo", "iuta", ")"]);
        let hit = spans
            .get(1)
            .ok_or_else(|| color_eyre::eyre::eyre!("应有命中段"))?;
        assert_eq!(
            hit.style.fg,
            Some(theme.search_hit_color),
            "别名命中与主字段同款换 search_hit 色"
        );
        assert!(
            hit.style.add_modifier.contains(theme.search_hit_modifier),
            "命中段叠 search_hit 字体效果"
        );
        for i in [0usize, 2, 3] {
            let dim = spans
                .get(i)
                .ok_or_else(|| color_eyre::eyre::eyre!("应有第 {i} 段"))?;
            assert_eq!(
                dim.style.fg,
                Some(theme.overlay),
                "括号与未命中字符保持 overlay 暗调"
            );
        }
        Ok(())
    }
}
