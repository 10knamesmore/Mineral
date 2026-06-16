//! 单行文本 + 光标的纯编辑逻辑(零 ratatui 依赖,CJK 安全)。
//!
//! 事件解码([`KeyEvent`](crossterm::event::KeyEvent) → [`InputRequest`])在调用方边缘,
//! 本结构只做纯态更新——合「边缘结构化、核心纯函数」,可独立单测、与 config-driven
//! keymap 不耦合。channel 搜索 prompt 与默认界面 `/` 模糊框共用同款行为。

/// 一次文本编辑意图。调用方把按键解码成它,[`LineInput`] 据此纯更新。
#[derive(Clone, Copy)]
pub(crate) enum InputRequest {
    /// 在光标处插入字符。
    Insert(char),

    /// 删光标前一字符(退格)。
    DeletePrev,

    /// 光标左移一格(钳词首)。
    Left,

    /// 光标右移一格(钳词尾)。
    Right,

    /// 光标跳词首。
    Home,

    /// 光标跳词尾。
    End,
}

/// 单行文本 + 光标(char 索引)的纯编辑态。
pub(crate) struct LineInput {
    /// 文本。
    text: String,

    /// 光标的 char 索引(`0..=字符数`):插入 / 退格作用于此处。
    cursor: usize,
}

impl LineInput {
    /// 新建空输入(光标在 0)。
    pub(crate) fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
        }
    }

    /// 应用一次编辑意图。
    ///
    /// # Params:
    ///   - `req`: 编辑意图
    ///
    /// # Return:
    ///   文本是否真的变了(插入 / 有效退格为 `true`;纯移动 / 词首退格为 `false`,
    ///   调用方据此决定要不要作废下游缓存)。
    pub(crate) fn apply(&mut self, req: InputRequest) -> bool {
        match req {
            InputRequest::Insert(c) => {
                let at = self.cursor_byte();
                self.text.insert(at, c);
                self.cursor = self.cursor.saturating_add(1);
                true
            }
            InputRequest::DeletePrev => {
                if self.cursor == 0 {
                    return false;
                }
                self.cursor = self.cursor.saturating_sub(1);
                let at = self.cursor_byte();
                self.text.remove(at);
                true
            }
            InputRequest::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                false
            }
            InputRequest::Right => {
                self.cursor = self.cursor.saturating_add(1).min(self.char_count());
                false
            }
            InputRequest::Home => {
                self.cursor = 0;
                false
            }
            InputRequest::End => {
                self.cursor = self.char_count();
                false
            }
        }
    }

    /// 当前文本。
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// 文本是否为空。
    pub(crate) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// 清空文本并把光标归 0。
    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// 以光标为界切两段 `(光标前, 光标后)`(渲染光标块用;恒落 char 边界,CJK 安全)。
    pub(crate) fn split(&self) -> (&str, &str) {
        self.text.split_at(self.cursor_byte())
    }

    /// 当前光标的字节偏移(char 索引 → 字节;越界落词尾)。
    fn cursor_byte(&self) -> usize {
        self.text
            .char_indices()
            .nth(self.cursor)
            .map_or(self.text.len(), |(b, _)| b)
    }

    /// 当前字符数。
    fn char_count(&self) -> usize {
        self.text.chars().count()
    }

    /// 测试构造:一次性灌入整段文本、光标落词尾。
    #[cfg(test)]
    pub(crate) fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.char_count();
    }
}

#[cfg(test)]
mod tests {
    use super::{InputRequest, LineInput};

    /// 插入落在光标处、光标随之右移;`split` 以光标为界切两段。
    #[test]
    fn insert_at_cursor_and_split() {
        let mut input = LineInput::new();
        assert!(input.apply(InputRequest::Insert('a')), "插入返回变更");
        input.apply(InputRequest::Insert('b'));
        assert_eq!(input.split(), ("ab", ""), "插入后光标在词尾");
        input.apply(InputRequest::Left);
        assert_eq!(input.split(), ("a", "b"), "左移落 a|b");
        input.apply(InputRequest::Insert('X'));
        assert_eq!(input.text(), "aXb", "插入落在光标处而非词尾");
        assert_eq!(input.split(), ("aX", "b"), "插入后光标停在新字符之后");
    }

    /// 退格删光标前一字符并返回 `true`;词首退格无字可删返回 `false`、不改文本。
    #[test]
    fn backspace_deletes_before_cursor_and_reports_change() {
        let mut input = LineInput::new();
        input.apply(InputRequest::Insert('a'));
        input.apply(InputRequest::Insert('b'));
        assert!(input.apply(InputRequest::DeletePrev), "退格删 b 返回 true");
        assert_eq!(input.text(), "a", "删掉的是光标前一字符");
        input.apply(InputRequest::Home);
        assert!(
            !input.apply(InputRequest::DeletePrev),
            "词首退格无字可删返回 false"
        );
        assert_eq!(input.text(), "a", "词首退格不改文本");
    }

    /// 光标移动键钳边且不改文本(返回 `false`):Right 越界钳词尾、Home/End 跳两端。
    #[test]
    fn cursor_moves_clamp_and_report_no_change() {
        let mut input = LineInput::new();
        input.apply(InputRequest::Insert('a'));
        input.apply(InputRequest::Insert('b'));
        assert!(!input.apply(InputRequest::Right), "右移不改文本返回 false");
        assert_eq!(input.split(), ("ab", ""), "右移越界钳词尾");
        input.apply(InputRequest::Home);
        assert_eq!(input.split(), ("", "ab"), "Home 跳词首");
        input.apply(InputRequest::End);
        assert_eq!(input.split(), ("ab", ""), "End 跳词尾");
    }

    /// 多字节(CJK)光标:byte 偏移按 char 边界,`split` 不切坏字符,中间插入安全。
    #[test]
    fn multibyte_safe() {
        let mut input = LineInput::new();
        for c in "周杰伦".chars() {
            input.apply(InputRequest::Insert(c));
        }
        input.apply(InputRequest::Left);
        assert_eq!(input.split(), ("周杰", "伦"), "光标落在 char 边界");
        input.apply(InputRequest::Insert('a'));
        assert_eq!(input.text(), "周杰a伦", "多字节中间插入不切坏字符");
    }

    /// `is_empty` 跟随文本;空输入态各操作不溢出。
    #[test]
    fn empty_state_is_safe() {
        let mut input = LineInput::new();
        assert!(input.is_empty(), "新建为空");
        assert!(!input.apply(InputRequest::DeletePrev), "空退格 no-op");
        assert!(!input.apply(InputRequest::Left), "空左移 no-op");
        input.apply(InputRequest::Insert('a'));
        assert!(!input.is_empty(), "插入后非空");
    }
}
