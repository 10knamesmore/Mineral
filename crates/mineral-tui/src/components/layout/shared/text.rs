//! 文本渲染小工具：CJK 双宽感知的字符 / 字符串宽度、歌名别名后缀 span。

use ratatui::style::Style;
use ratatui::text::Span;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::render::theme::Theme;

/// 字符串显示宽度（CJK 双宽）；溢出 u16 夹到 MAX。
pub(crate) fn display_width(s: &str) -> u16 {
    u16::try_from(UnicodeWidthStr::width(s)).unwrap_or(u16::MAX)
}

/// 单字符显示宽度（控制字符按 0）。
pub(crate) fn char_width(ch: char) -> u16 {
    u16::try_from(UnicodeWidthChar::width(ch).unwrap_or(0)).unwrap_or(u16::MAX)
}

/// 歌名别名后缀 span：` (alias)`，overlay 暗色（比歌名暗一档，与计量列同层）。
///
/// # Params:
///   - `alias`: [`Song::alias`](mineral_model::Song)，`None` 时无后缀
///
/// # Return:
///   可直接 `extend` 进 title spans 的后缀；无别名给 `None`。
pub(crate) fn alias_span(alias: Option<&str>, theme: &Theme) -> Option<Span<'static>> {
    alias.map(|a| Span::styled(format!(" ({a})"), Style::new().fg(theme.overlay)))
}

#[cfg(test)]
mod tests {
    use super::alias_span;
    use crate::render::theme::Theme;

    /// alias_span:有别名给 ` (alias)`(前导分隔空格 + 括号)、overlay 暗色;无别名给 `None`。
    /// 在 span 文本层锁住分隔符 / 括号 / 颜色——CJK 双宽在 buffer 层会插补位空格,故不走渲染断言。
    #[test]
    fn alias_span_wraps_with_leading_space_and_dim() -> color_eyre::Result<()> {
        let theme = Theme::default();
        assert!(alias_span(None, &theme).is_none(), "无别名应 None");

        let span = alias_span(Some("Mayoiuta"), &theme)
            .ok_or_else(|| color_eyre::eyre::eyre!("有别名应 Some"))?;
        assert_eq!(span.content, " (Mayoiuta)", "应带前导分隔空格与括号");
        assert_eq!(
            span.style.fg,
            Some(theme.overlay),
            "别名后缀应 overlay 暗色"
        );
        Ok(())
    }
}
