//! 展示层数值格式化(时长 → `m:ss`)。TUI 各面板共用一处口径,避免多份副本漂移。

/// 时长 `m:ss` 格式化(ms 输入,分钟不补零、秒补零)。
///
/// # Params:
///   - `ms`: 时长毫秒
///
/// # Return:
///   `m:ss` 串(如 `1:05`、`61:01`)。
pub fn format_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let m = secs / 60;
    let s = secs % 60;
    format!("{m}:{s:02}")
}

/// 时长 `m:ss` 格式化;未知(`None`)画 `-:--` 占位(与真实 `0:00` 区分,宽度同最小 `m:ss`)。
///
/// # Params:
///   - `ms`: 时长毫秒,`None` = 未知
///
/// # Return:
///   有值给 `m:ss`,`None` 给 `-:--`。
pub fn format_ms_opt(ms: Option<u64>) -> String {
    ms.map_or_else(|| "-:--".to_owned(), format_ms)
}

#[cfg(test)]
mod tests {
    use super::{format_ms, format_ms_opt};

    /// `format_ms`:秒 / 分进位与零填充。
    #[test]
    fn format_ms_cases() {
        assert_eq!(format_ms(0), "0:00");
        assert_eq!(format_ms(75_000), "1:15");
        assert_eq!(format_ms(3_661_000), "61:01");
    }

    /// `format_ms_opt`:未知画 `-:--`,与真实 `0:00` 区分。
    #[test]
    fn format_ms_opt_unknown_is_placeholder() {
        assert_eq!(format_ms_opt(None), "-:--");
        assert_eq!(format_ms_opt(Some(0)), "0:00");
        assert_eq!(format_ms_opt(Some(65_000)), "1:05");
    }
}
