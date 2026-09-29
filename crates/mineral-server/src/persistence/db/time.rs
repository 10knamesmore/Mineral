//! 时间工具。

/// 当前 unix 毫秒。溢出/系统时间倒退时给安全兜底(不 panic)。
///
/// # Return:
///   自 epoch 的毫秒数；不可表示时返回 `i64::MAX`，倒退时返回 0。
pub(crate) fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}
