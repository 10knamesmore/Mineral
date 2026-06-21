//! detail 头部的简介（album / artist / playlist）多行渲染与滚动视口。
//!
//! 数据层简介按 `\n` 保留原始换行与空行（作者有意的段落间隔）；这里把它拆成逻辑行、再按
//! 显示宽度词感知折行成可视行，给一个 `Cell` 维护的滚动 offset 开窗口。拉丁词整体不拆，
//! 无空格的长 CJK 串按列逐字符断。

use std::cell::Cell;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use super::text::{char_width, display_width};
use crate::render::theme::Theme;

/// 把简介原文折成可视行序列：先按 `\n` 拆逻辑行（空 / 纯空白行原样保留，不压不并），
/// 每条逻辑行再按显示宽度 `width` 词感知折行。`text` 空或 `width==0` → 空序列。
///
/// # Params:
///   - `text`: 简介原文（含 `\n`）
///   - `width`: 可视行宽（列）
///
/// # Return:
///   折行后的可视行（每条 ≤ `width` 列）。
pub(crate) fn wrap_description(text: &str, width: u16) -> Vec<String> {
    if text.is_empty() || width == 0 {
        return Vec::new();
    }
    let mut rows = Vec::<String>::new();
    for logical in text.split('\n') {
        wrap_logical(logical, width, &mut rows);
    }
    rows
}

/// 折一条逻辑行进 `rows`：纯空白 / 空行原样保真为一条（作者有意的段落间隔，含「单空格」
/// spacer）；否则按空格切词、词感知贪心折行——拉丁词整体不拆，超过整行宽的词（无空格的长
/// CJK 串 / 超长英文）才逐字符硬断。
fn wrap_logical(line: &str, width: u16, rows: &mut Vec<String>) {
    if line.chars().all(char::is_whitespace) {
        rows.push(line.to_owned());
        return;
    }
    let mut row = String::new();
    let mut row_w = 0u16;
    for word in line.split(' ') {
        // 连续 / 首尾空格产生的空 word：跳过（行内多空格归一为一个，由词间补空格还原）。
        if word.is_empty() {
            continue;
        }
        let ww = display_width(word);
        if ww > width {
            // 词本身就宽过整行：先冲掉当前行，再逐字符硬断（CJK 串走这条 = 逐字符断）。
            if !row.is_empty() {
                rows.push(std::mem::take(&mut row));
                row_w = 0;
            }
            hard_break(word, width, rows, &mut row, &mut row_w);
            continue;
        }
        // 普通词：放不下就换行（换行后行首不补空格）；放得下且行非空则补一个词间空格。
        let need = if row.is_empty() {
            ww
        } else {
            ww.saturating_add(1)
        };
        if row_w.saturating_add(need) > width {
            rows.push(std::mem::take(&mut row));
            row.push_str(word);
            row_w = ww;
        } else {
            if !row.is_empty() {
                row.push(' ');
                row_w = row_w.saturating_add(1);
            }
            row.push_str(word);
            row_w = row_w.saturating_add(ww);
        }
    }
    if !row.is_empty() {
        rows.push(row);
    }
}

/// 把超长词逐字符硬断进 `rows`，尾部不满一行的残段留在 `row`/`row_w`（供后续词接续）。
fn hard_break(word: &str, width: u16, rows: &mut Vec<String>, row: &mut String, row_w: &mut u16) {
    for ch in word.chars() {
        let w = char_width(ch);
        // 单字符就宽过整行（极端：双宽字符撞上 width==1）：自成一行，防死循环。
        if w > width {
            if !row.is_empty() {
                rows.push(std::mem::take(row));
                *row_w = 0;
            }
            rows.push(ch.to_string());
            continue;
        }
        if row_w.saturating_add(w) > width {
            rows.push(std::mem::take(row));
            *row_w = 0;
        }
        row.push(ch);
        *row_w = row_w.saturating_add(w);
    }
}

/// 把滚动 `offset` 钳进 `[0, max(0, total - viewport)]`（`total`/`viewport` 为可视行数）。
pub(crate) fn clamp_scroll(offset: u16, total: u16, viewport: u16) -> u16 {
    offset.min(total.saturating_sub(viewport))
}

/// 画简介滚动视口：折行 → 按 `scroll` 开窗 → 末列溢出时画滚动条。`scroll` 在此被钳进内容
/// 边界并**写回**（render 走 `&self`，故内部可变；消除过滚后反向的死区）。`text` 空 / 区域
/// 退化时不画。
///
/// # Params:
///   - `area`: 简介区（含末列滚动条位）
///   - `text`: 简介原文
///   - `scroll`: 滚动 offset（可视行）；钳后写回
pub(crate) fn draw_description(
    buf: &mut Buffer,
    area: Rect,
    text: &str,
    scroll: &Cell<u16>,
    theme: &Theme,
) {
    if area.height == 0 || area.width == 0 || text.is_empty() {
        return;
    }
    // 末列留给滚动条（恒定预留，版面不随是否溢出抖动）。
    let text_w = area.width.saturating_sub(1).max(1);
    let rows = wrap_description(text, text_w);
    let total = u16::try_from(rows.len()).unwrap_or(u16::MAX);
    let viewport = area.height;
    let off = clamp_scroll(scroll.get(), total, viewport);
    scroll.set(off);
    let dim = Style::new().fg(theme.overlay);
    let visible = rows
        .iter()
        .skip(usize::from(off))
        .take(usize::from(viewport))
        .map(|r| Line::from(Span::styled(r.clone(), dim)))
        .collect::<Vec<Line<'static>>>();
    let text_area = Rect::new(area.x, area.y, text_w, area.height);
    Widget::render(Paragraph::new(visible), text_area, buf);
    if total > viewport {
        draw_scrollbar(buf, area, total, viewport, off, theme);
    }
}

/// 纵向滚动条滑块几何（纯函数，便于回归测试）：返回 `(滑块顶端行偏移, 滑块长)`。
///
/// 滑块长只随 `viewport`/`total`（**不含 `off`**，故滚动时长度恒定、不蠕动）；`off=0` 顶端=0
/// （贴顶）、`off=max_off`（=`total-viewport`）时 `顶端+长==track_len`（底边贴轨道底）。
/// `track_len`/`total` 为 0 → `(0, 0)`。
fn scrollbar_thumb(total: u16, viewport: u16, off: u16, track_len: u16) -> (u16, u16) {
    if track_len == 0 || total == 0 {
        return (0, 0);
    }
    // 滑块长 = viewport 占 total 的比例 × 轨道高，夹到 [1, 轨道高];只随内容/视口定、不随 off。
    let thumb_len = u16::try_from(u32::from(viewport) * u32::from(track_len) / u32::from(total))
        .unwrap_or(track_len)
        .clamp(1, track_len);
    let travel = track_len.saturating_sub(thumb_len);
    let max_off = total.saturating_sub(viewport);
    let thumb_top = if max_off == 0 {
        0
    } else {
        u16::try_from(u32::from(off) * u32::from(travel) / u32::from(max_off)).unwrap_or(travel)
    };
    (thumb_top, thumb_len)
}

/// 在 `area` 末列画定长滑块的纵向滚动条（几何见 [`scrollbar_thumb`]）。仅溢出
/// （`total > viewport`）时调用。
fn draw_scrollbar(
    buf: &mut Buffer,
    area: Rect,
    total: u16,
    viewport: u16,
    off: u16,
    theme: &Theme,
) {
    let track_len = area.height;
    if track_len == 0 || area.width == 0 {
        return;
    }
    let (thumb_top, thumb_len) = scrollbar_thumb(total, viewport, off, track_len);
    let x = area.x.saturating_add(area.width).saturating_sub(1);
    let thumb = Style::new().fg(theme.subtext);
    let track = Style::new().fg(theme.overlay);
    for i in 0..track_len {
        let (sym, style) = if i >= thumb_top && i < thumb_top.saturating_add(thumb_len) {
            ("█", thumb)
        } else {
            ("│", track)
        };
        if let Some(cell) = buf.cell_mut((x, area.y.saturating_add(i))) {
            cell.set_symbol(sym).set_style(style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{clamp_scroll, scrollbar_thumb, wrap_description};

    /// 回归锁(用户实测两 bug):滚动条滑块长度恒定(不随 off「一长一短蠕动」)、到顶贴顶、
    /// **到底滑块底边正好贴轨道底**(此前到底了滑块还没到底、误导下面有内容)。
    #[test]
    fn scrollbar_thumb_constant_len_and_flush_bottom() {
        let (total, viewport, track) = (50u16, 10u16, 8u16);
        let max_off = total - viewport;
        let (top_0, len_0) = scrollbar_thumb(total, viewport, /*off*/ 0, track);
        let (_, len_mid) = scrollbar_thumb(total, viewport, max_off / 2, track);
        let (top_max, len_max) = scrollbar_thumb(total, viewport, max_off, track);
        // 长度恒定:三处一致 → 不蠕动。
        assert_eq!(len_0, len_mid, "滑块长度不随 off 变(中途)");
        assert_eq!(len_0, len_max, "滑块长度不随 off 变(到底)");
        // 到顶贴顶、到底贴底。
        assert_eq!(top_0, 0, "off=0 滑块贴顶");
        assert_eq!(
            top_max + len_max,
            track,
            "off=max 滑块底边贴轨道底(回归:到底滑块没到底)"
        );
    }

    /// 内容恰好不溢出(total==viewport):滑块满轨、贴顶(不会出现半截滑块)。
    #[test]
    fn scrollbar_thumb_fills_track_when_no_overflow() {
        let (top, len) = scrollbar_thumb(
            /*total*/ 8, /*viewport*/ 8, /*off*/ 0, /*track*/ 8,
        );
        assert_eq!((top, len), (0, 8), "不溢出时滑块满轨贴顶");
    }

    /// 换行保真：`\n` 拆成独立可视行（修「整段塞一个 Line、换行被吞」的 bug）。
    #[test]
    fn wrap_splits_on_newline() {
        assert_eq!(wrap_description("alpha\nbeta", 40), vec!["alpha", "beta"]);
    }

    /// 空行 / 纯空白行原样保留、不压不并（作者有意的段落间隔）。
    #[test]
    fn wrap_preserves_blank_lines() {
        // 连续空行不塌成一行。
        assert_eq!(
            wrap_description("a\n\n\nb", 40),
            vec!["a", "", "", "b"],
            "三个 \\n 给出两条空行,不合并"
        );
        // 网易云那种「只含一个空格」的 spacer 行保真。
        assert_eq!(wrap_description("a\n \nb", 40), vec!["a", " ", "b"]);
    }

    /// CJK 双宽断行：width=4 时每行恰两个中文字（每字 2 列）。
    #[test]
    fn wrap_cjk_by_display_width() {
        assert_eq!(
            wrap_description("胜利或失败", /*width*/ 4),
            vec!["胜利", "或失", "败"]
        );
    }

    /// 词感知折行：拉丁词整体不拆，放不下就整体换行（不再「Fo / otball」拦腰断）。
    #[test]
    fn wrap_keeps_latin_words_whole() {
        assert_eq!(
            wrap_description("Chinese Football", /*width*/ 8),
            vec!["Chinese", "Football"]
        );
    }

    /// 超长词（宽过整行）逐字符硬断，残段接续。
    #[test]
    fn wrap_hard_breaks_overlong_word() {
        assert_eq!(
            wrap_description("Footballer", /*width*/ 8),
            vec!["Football", "er"]
        );
    }

    /// 空原文 / 零宽 → 空序列（不渲染）。
    #[test]
    fn wrap_empty_yields_nothing() {
        assert!(wrap_description("", 40).is_empty());
        assert!(wrap_description("anything", 0).is_empty());
    }

    /// 滚动钳制：内容短于视口恒 0；溢出时上界 = total-viewport。
    #[test]
    fn clamp_scroll_bounds() {
        assert_eq!(clamp_scroll(/*offset*/ 5, /*total*/ 3, /*viewport*/ 10), 0);
        assert_eq!(clamp_scroll(100, 30, 10), 20, "上界 total-viewport");
        assert_eq!(clamp_scroll(7, 30, 10), 7, "界内不动");
    }
}
