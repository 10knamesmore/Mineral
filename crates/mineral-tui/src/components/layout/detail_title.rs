//! detail 面板顶栏 title 的 breadcrumb 组装：把详情栈每帧的 `(类型, 名)` 拼成
//! `图标 类型 · 名` / 多帧 `图标 名 › 图标 名`，并按面板显示宽度截断（CJK 双宽感知）。

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use mineral_model::SearchKind;

use crate::runtime::state::AppState;

/// detail 顶栏 title：当前结果集的详情栈 breadcrumb；无结果 / 空栈回退固定 `detail`。
/// `width` 是面板外框宽，扣掉圆角边框占位后按显示宽度截断。
pub fn for_panel(state: &AppState, width: u16) -> String {
    let budget = width.saturating_sub(4);
    match state.channel_search.active_results() {
        Some(kr) => frame_title(&kr.detail.title_crumbs(), budget),
        None => "detail".to_owned(),
    }
}

/// 详情栈 breadcrumb → 顶栏 title 文案，按显示宽度 `max_width` 截断。
///
/// root 单帧 → `图标 单数类型 · 名`；下钻多帧 → 各帧 `图标 名` 用 ` › ` 链接（图标代类型）。
/// 超宽时优先截最左祖先名、保当前帧名完整；当前帧那一节自身放不下才截它补 `…`。空链 → `detail`。
///
/// # Params:
///   - `crumbs`: 栈帧链（root→top）的 `(类型, 名)`
///   - `max_width`: 可用显示宽度（列）
///
/// # Return:
///   组装并按宽度截断后的 title 文案。
fn frame_title(crumbs: &[(SearchKind, &str)], max_width: u16) -> String {
    let Some((last, ancestors)) = crumbs.split_last() else {
        return "detail".to_owned();
    };
    let (last_kind, last_name) = *last;
    if ancestors.is_empty() {
        let prefix = format!("{} {} · ", last_kind.icon(), last_kind.singular());
        return fit_prefixed(&prefix, last_name, max_width);
    }
    let last_seg = format!("{} {}", last_kind.icon(), last_name);
    let head = ancestors
        .iter()
        .map(|(k, n)| format!("{} {}", k.icon(), n))
        .collect::<Vec<String>>()
        .join(" › ");
    let sep = " › ";
    let full = format!("{head}{sep}{last_seg}");
    if display_width(&full) <= max_width {
        return full;
    }
    let reserve = display_width(&last_seg).saturating_add(display_width(sep));
    if reserve >= max_width {
        // 连当前帧那一节都放不下：退化为只截当前帧名（保图标）。
        let prefix = format!("{} ", last_kind.icon());
        return fit_prefixed(&prefix, last_name, max_width);
    }
    let head_trunc = truncate_to_width(&head, max_width.saturating_sub(reserve));
    format!("{head_trunc}{sep}{last_seg}")
}

/// `前缀 + 名` 放不下时只截名补 `…`（保前缀）；前缀本身就超宽则整体截断兜底。
fn fit_prefixed(prefix: &str, name: &str, max_width: u16) -> String {
    let full = format!("{prefix}{name}");
    if display_width(&full) <= max_width {
        return full;
    }
    let pw = display_width(prefix);
    if pw < max_width {
        let name = truncate_to_width(name, max_width.saturating_sub(pw));
        return format!("{prefix}{name}");
    }
    truncate_to_width(&full, max_width)
}

/// 按显示宽度截断到 `max_width`，截掉则补 `…`（占 1 列）；本就够宽原样返回。
fn truncate_to_width(s: &str, max_width: u16) -> String {
    if display_width(s) <= max_width {
        return s.to_owned();
    }
    let budget = max_width.saturating_sub(1); // 给省略号留 1 列
    let mut acc = 0u16;
    let mut out = String::new();
    for ch in s.chars() {
        let w = char_width(ch);
        if acc.saturating_add(w) > budget {
            break;
        }
        acc = acc.saturating_add(w);
        out.push(ch);
    }
    out.push('…');
    out
}

/// 字符串显示宽度（CJK 双宽）；溢出 u16 夹到 MAX。
fn display_width(s: &str) -> u16 {
    u16::try_from(UnicodeWidthStr::width(s)).unwrap_or(u16::MAX)
}

/// 单字符显示宽度（控制字符按 0）。
fn char_width(ch: char) -> u16 {
    u16::try_from(UnicodeWidthChar::width(ch).unwrap_or(0)).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use mineral_model::SearchKind;

    use super::frame_title;

    /// root 帧（单节）：`图标 单数类型 · 名`。
    #[test]
    fn title_root_frame_is_kind_and_name() {
        assert_eq!(
            frame_title(&[(SearchKind::Album, "范特西")], /*max_width*/ 40),
            "◉ album · 范特西"
        );
        assert_eq!(
            frame_title(&[(SearchKind::Playlist, "Chill")], 40),
            "▤ playlist · Chill"
        );
    }

    /// 下钻帧（多节）：图标代类型、名用 ` › ` 链接，宽度够时全展开。
    #[test]
    fn title_breadcrumb_joins_crumbs() {
        assert_eq!(
            frame_title(
                &[
                    (SearchKind::Artist, "周杰伦"),
                    (SearchKind::Album, "范特西")
                ],
                40,
            ),
            "✦ 周杰伦 › ◉ 范特西"
        );
    }

    /// 超宽：按显示宽度截祖先名（CJK 双宽）、当前帧名保持完整。
    #[test]
    fn title_breadcrumb_truncates_ancestor_keeps_current() {
        // 全长 "✦ 周杰伦 › ◉ 范特西" 显示宽 19；给 17 容不下 → 截祖先到 "✦ 周…"。
        assert_eq!(
            frame_title(
                &[
                    (SearchKind::Artist, "周杰伦"),
                    (SearchKind::Album, "范特西")
                ],
                17,
            ),
            "✦ 周… › ◉ 范特西"
        );
    }

    /// 当前帧名自己都放不下时：截当前帧名补省略号（祖前缀保留图标/类型词）。
    #[test]
    fn title_root_truncates_long_name() {
        // 前缀 "◉ album · " 宽 10；给 14 → 名字预算 4 → "范特西精选" 截成 "范…"。
        assert_eq!(
            frame_title(&[(SearchKind::Album, "范特西精选")], 14),
            "◉ album · 范…"
        );
    }

    /// 空链回退固定 `detail`（无实体可标）。
    #[test]
    fn title_empty_falls_back() {
        assert_eq!(frame_title(&[], 40), "detail");
    }
}
