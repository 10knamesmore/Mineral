//! 行内文本光标:把「光标前 / 后文本」渲染成带反色光标的 [`Span`] 序列。
//!
//! 光标不是夹在两段之间的独立 `█` 块(会把后文推开一格),而是反色覆盖在光标后第一个字符上
//! ——视觉上光标"住进"字符里(`ab[c]d`),与终端原生块光标一致;光标落词尾时无字可罩,改罩
//! 一个空格(`abc█`),与原词尾块光标视觉一致。反色(前景色实心块、字挖空成背景色)不依赖主题
//! 背景字段,自动适配每处调用的 base 前景色,故 channel 搜索输入框与 browse 侧栏 `/` 搜索 badge
//! 共用此渲染,免得两处各画一份、各自漂移。

use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

/// 把以光标为界的两段文本渲染成带行内反色光标的 Span 序列。
///
/// # Params:
///   - `before`: 光标前文本(调用方已按需加前缀,如 badge 的 `/`);空则不占段
///   - `after`: 光标后文本;其首字符被光标反色罩住,光标落词尾(`after` 空)时改罩一个空格
///   - `base`: 文本基样式;光标段在此基础上叠 [`Modifier::REVERSED`](前景色实心块、字挖空成背景色)
///
/// # Return:
///   `[before, 光标段, after 余下]` 三段 Span(空段省略),可直接拼进 [`Line`](ratatui::text::Line)。
pub(crate) fn cursor_spans(before: String, after: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::<Span<'static>>::new();
    if !before.is_empty() {
        spans.push(Span::styled(before, base));
    }
    let cursor = base.add_modifier(Modifier::REVERSED);
    let mut chars = after.chars();
    match chars.next() {
        // 光标罩住 after 首字符;`Chars::as_str` 取余下未迭代部分(零拷贝,不触 indexing_slicing)。
        Some(first) => {
            let rest = chars.as_str();
            spans.push(Span::styled(first.to_string(), cursor));
            if !rest.is_empty() {
                spans.push(Span::styled(rest.to_owned(), base));
            }
        }
        // 光标落词尾:无字可罩,反色一个空格占位。
        None => spans.push(Span::styled(" ", cursor)),
    }
    spans
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Span;

    use super::cursor_spans;

    /// 收集各段文本,便于断言切分形态。
    fn contents<'a>(spans: &'a [Span<'static>]) -> Vec<&'a str> {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// 光标落词中:after 首字符单独成段被反色罩住,前后两段保持 base 样式。
    #[test]
    fn cursor_covers_first_after_char() -> color_eyre::Result<()> {
        let spans = cursor_spans("ab".to_owned(), "cd", Style::new());
        assert_eq!(contents(&spans), ["ab", "c", "d"], "切成 before|光标字符|余下");
        let cursor = spans.get(1).ok_or_else(|| color_eyre::eyre::eyre!("缺光标段"))?;
        assert!(
            cursor.style.add_modifier.contains(Modifier::REVERSED),
            "光标段反色"
        );
        Ok(())
    }

    /// 光标落词尾:无字可罩,反色一个空格占位。
    #[test]
    fn cursor_at_end_covers_space() -> color_eyre::Result<()> {
        let spans = cursor_spans("abc".to_owned(), "", Style::new());
        assert_eq!(contents(&spans), ["abc", " "], "词尾光标罩空格");
        let cursor = spans.get(1).ok_or_else(|| color_eyre::eyre::eyre!("缺光标段"))?;
        assert!(
            cursor.style.add_modifier.contains(Modifier::REVERSED),
            "空格光标段反色"
        );
        Ok(())
    }

    /// 光标落词首:before 为空不占段,首字符直接成光标段。
    #[test]
    fn cursor_at_start_omits_empty_before() {
        let spans = cursor_spans(String::new(), "ab", Style::new());
        assert_eq!(contents(&spans), ["a", "b"], "空 before 省略");
    }

    /// 多字节(CJK):光标整罩一个宽字符,不切坏字符、无空余下段。
    #[test]
    fn cursor_covers_full_wide_char() {
        let spans = cursor_spans("周杰".to_owned(), "伦", Style::new());
        assert_eq!(contents(&spans), ["周杰", "伦"], "宽字符整罩、无余下");
    }
}
