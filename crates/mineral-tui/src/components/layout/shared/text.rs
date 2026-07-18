//! 文本渲染小工具：CJK 双宽感知的字符 / 字符串宽度、歌名别名后缀 span、
//! 渲染面实际背景采样。

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// 字符串显示宽度（CJK 双宽）；溢出 u16 夹到 MAX。
pub(crate) fn display_width(s: &str) -> u16 {
    u16::try_from(UnicodeWidthStr::width(s)).unwrap_or(u16::MAX)
}

/// 单字符显示宽度（控制字符按 0）。
pub(crate) fn char_width(ch: char) -> u16 {
    u16::try_from(UnicodeWidthChar::width(ch).unwrap_or(0)).unwrap_or(u16::MAX)
}

/// 歌名别名后缀 span：` (alias)`，暗色（比歌名暗一档，与计量列同层）。
///
/// # Params:
///   - `alias`: [`Song::alias`](mineral_model::Song)，`None` 时无后缀
///   - `color`: 后缀色（固定底面板给 `theme.overlay`，氛围背景面给 `ink.muted`）
///
/// # Return:
///   可直接 `extend` 进 title spans 的后缀；无别名给 `None`。
pub(crate) fn alias_span(alias: Option<&str>, color: Color) -> Option<Span<'static>> {
    alias.map(|a| Span::styled(format!(" ({a})"), Style::new().fg(color)))
}

/// 读区域中心 cell 的**实际**背景色（本渲染面先铺 bg、后画字的采样点，喂
/// [`crate::render::theme::Theme::ink_over`]）。区域为空 / 越界给 `Color::Reset`，
/// 下游按「拿不到真彩」回落静态 token——固定底色面板与氛围背景面由此走同一条取色路径。
pub(crate) fn center_bg(frame: &mut Frame<'_>, area: Rect) -> Color {
    let x = area.x.saturating_add(area.width / 2);
    let y = area.y.saturating_add(area.height / 2);
    frame
        .buffer_mut()
        .cell(Position::new(x, y))
        .map_or(Color::Reset, |cell| cell.bg)
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::alias_span;

    /// alias_span:有别名给 ` (alias)`(前导分隔空格 + 括号)、着调用方给的暗色;无别名给 `None`。
    /// 在 span 文本层锁住分隔符 / 括号 / 颜色——CJK 双宽在 buffer 层会插补位空格,故不走渲染断言。
    #[test]
    fn alias_span_wraps_with_leading_space_and_dim() -> color_eyre::Result<()> {
        let dim = Color::Rgb(0x6c, 0x70, 0x86);
        assert!(alias_span(None, dim).is_none(), "无别名应 None");

        let span = alias_span(Some("Mayoiuta"), dim)
            .ok_or_else(|| color_eyre::eyre::eyre!("有别名应 Some"))?;
        assert_eq!(span.content, " (Mayoiuta)", "应带前导分隔空格与括号");
        assert_eq!(span.style.fg, Some(dim), "别名后缀应着传入的暗色");
        Ok(())
    }
}
