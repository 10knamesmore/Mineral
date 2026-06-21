//! 终端显示宽度小工具：CJK 双宽感知的字符 / 字符串宽度。

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// 字符串显示宽度（CJK 双宽）；溢出 u16 夹到 MAX。
pub(crate) fn display_width(s: &str) -> u16 {
    u16::try_from(UnicodeWidthStr::width(s)).unwrap_or(u16::MAX)
}

/// 单字符显示宽度（控制字符按 0）。
pub(crate) fn char_width(ch: char) -> u16 {
    u16::try_from(UnicodeWidthChar::width(ch).unwrap_or(0)).unwrap_or(u16::MAX)
}
