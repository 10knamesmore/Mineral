//! 可复用单行输入：字符编辑、粘贴与随光标移动的显示窗口。

use crate::render::cursor::cursor_spans;
use crate::runtime::line_input::{InputRequest, LineInput};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::Line,
    widgets::{Paragraph, Widget},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// 持有单行编辑状态并绘制可见部分。
pub(crate) struct TextInput {
    /// 文本和字符光标。
    input: LineInput,
}

impl TextInput {
    /// 创建空输入。
    pub(crate) fn new() -> Self {
        Self {
            input: LineInput::new(),
        }
    }
    /// 返回完整编辑文本。
    pub(crate) fn text(&self) -> &str {
        self.input.text()
    }
    /// 预填文本并将光标放在末尾。
    pub(crate) fn set_text(&mut self, text: String) {
        self.input.set_text(text);
    }

    /// 换行与制表符变为空格，其他控制字符丢弃；粘贴不触发动作。
    pub(crate) fn paste(&mut self, text: &str) {
        for c in text.chars() {
            if matches!(c, '\n' | '\r' | '\t') {
                self.input.apply(InputRequest::Insert(' '));
            } else if !c.is_control() {
                self.input.apply(InputRequest::Insert(c));
            }
        }
    }

    /// 处理字符与编辑键，忽略动作组合键。
    pub(crate) fn on_key(&mut self, key: &KeyEvent) {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return;
        }
        let request = match key.code {
            KeyCode::Char(c) if !c.is_control() => InputRequest::Insert(c),
            KeyCode::Backspace => InputRequest::DeletePrev,
            KeyCode::Left => InputRequest::Left,
            KeyCode::Right => InputRequest::Right,
            KeyCode::Home => InputRequest::Home,
            KeyCode::End => InputRequest::End,
            _ => return,
        };
        self.input.apply(request);
    }

    /// 按字符边界截取输入，让光标留在给定列宽内。
    pub(crate) fn render(&self, buf: &mut Buffer, area: Rect, style: Style) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let (before, after) = self.input.split();
        let (before, after) = visible_window(before, after, usize::from(area.width));
        Paragraph::new(Line::from(cursor_spans(before.to_owned(), after, style))).render(area, buf);
    }
}

/// 留出完整光标字符的列宽，再从左侧按字符边界裁去超出的前缀。
fn visible_window<'a>(before: &'a str, after: &'a str, width: usize) -> (&'a str, &'a str) {
    let cursor_width = after
        .chars()
        .next()
        .map_or(1, |c| c.width().unwrap_or(0).max(1));
    let budget = width.saturating_sub(cursor_width);
    let mut start = 0;
    let mut used = before.width();
    for (index, c) in before.char_indices() {
        if used <= budget {
            break;
        }
        used = used.saturating_sub(c.width().unwrap_or(0));
        start = index + c.len_utf8();
    }
    // 单列窗口容不下宽字符时用词尾光标，避免半个中文字符。
    (
        before.get(start..).unwrap_or_default(),
        if cursor_width > width { "" } else { after },
    )
}

#[cfg(test)]
mod tests {
    use super::visible_window;
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    /// UTF-8 切片与终端列宽是输入控件的边界契约。
    #[test]
    fn scrolling_keeps_cursor_inside_narrow_unicode_windows() {
        for text in ["abc夜跑🙂", "长名称长名称", "e\u{301}你好"] {
            for boundary in text
                .char_indices()
                .map(|(index, _)| index)
                .chain(std::iter::once(text.len()))
            {
                let (before, after) = text.split_at(boundary);
                for width in 1..12 {
                    let (left, right) = visible_window(before, after, width);
                    assert!(before.ends_with(left));
                    assert!(right.is_empty() || right == after);
                    let cursor = right
                        .chars()
                        .next()
                        .map_or(1, |c| c.width().unwrap_or(0).max(1));
                    assert!(left.width() + cursor <= width);
                }
            }
        }
    }
}
