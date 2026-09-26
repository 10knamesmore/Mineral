//! 列表行内的搜索命中高亮:按 char 下标切片成 Span 序列。
//!
//! 供任意展示模糊过滤结果的列表面共用,避免高亮规则在多处漂移。
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
/// 别名括注样式收敛此一处,凡渲染歌名后缀的列表面共用。
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
