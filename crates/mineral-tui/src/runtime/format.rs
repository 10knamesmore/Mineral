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

/// 多曲累计时长的粗粒度格式化(`3h 3m` / `48m` / `45s`)。
///
/// 与 [`format_ms`] 分开是因为口径不同:单曲要秒级精度,累计时长动辄破小时,`m:ss`
/// 会给出 `183:24` 这种读不出来的串。
///
/// # Params:
///   - `ms`: 累计毫秒
///
/// # Return:
///   有小时给 `Xh Ym`(整点时省去 `Ym`),不足一分钟给 `Xs`(队列快见底时精确到秒),
///   其余给 `Xm`。
pub fn format_total(ms: u64) -> String {
    let minutes = ms / 60_000;
    let (h, m) = (minutes / 60, minutes % 60);
    match (h, m) {
        (0, 0) => format!("{}s", ms / 1000),
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// 一组时长的累计,以及其中有多少项时长未知。
///
/// 未知项**跳过**而不是当 0 累加:后者会静默把「不知道」说成「零秒」,让总时长比实际短
/// 且无从察觉。调用方据 `unknown` 决定要不要给结果加个「至少」的记号。
///
/// # Params:
///   - `durations`: 各曲时长,`None` = 未知
///
/// # Return:
///   `(已知项累计毫秒, 未知项个数)`。
pub fn sum_durations(durations: impl IntoIterator<Item = Option<u64>>) -> (u64, usize) {
    durations
        .into_iter()
        .fold((0, 0), |(total, unknown), each| match each {
            Some(ms) => (total.saturating_add(ms), unknown),
            None => (total, unknown.saturating_add(1)),
        })
}

/// 本地钟点 `HH:MM`;若目标日期比基准日期晚,加 `+Nd` 后缀(队列跨夜播完时避免歧义)。
///
/// # Params:
///   - `base`: 基准时刻(通常「现在」)
///   - `at`: 目标时刻(通常「预计播完」)
///
/// # Return:
///   同日给 `HH:MM`;跨日给 `HH:MM +Nd`。
pub fn format_clock(
    base: chrono::DateTime<chrono::Local>,
    at: chrono::DateTime<chrono::Local>,
) -> String {
    use chrono::Timelike;
    let clock = format!("{:02}:{:02}", at.hour(), at.minute());
    let days = at
        .date_naive()
        .signed_duration_since(base.date_naive())
        .num_days();
    if days > 0 {
        format!("{clock} +{days}d")
    } else {
        clock
    }
}

#[cfg(test)]
mod tests {
    use super::sum_durations;

    /// `sum_durations`:未知项跳过并单独计数,不静默当 0 累加。
    #[test]
    fn sum_durations_skips_unknown_and_counts_them() {
        let (total, unknown) = sum_durations([Some(1000), None, Some(2000), None]);
        assert_eq!(total, 3000);
        assert_eq!(unknown, 2);
    }
}
