//! queue 浮层的底栏标签:左下光标位置,右下剩余曲目数、时长与预计播完钟点。

use crate::runtime::format::{format_clock, format_total, sum_durations};
use crate::runtime::state::AppState;

/// 底栏放得下剩余信息所需的最小内区宽;窄于此右下留空。
const REMAINING_MIN_W: u16 = 34;

/// 左下标签:`3 / 12`。空队列给 `0 / 0`。
///
/// # Params:
///   - `sel`: 光标下标
///   - `ctx`: 应用状态(读队列长度)
///
/// # Return:
///   位置文本(两端各留一个空格,与边框标题的既有留白一致)。
pub(super) fn position_label(sel: usize, ctx: &AppState) -> String {
    let total = ctx.player.queue.len();
    if total == 0 {
        return " 0 / 0 ".to_owned();
    }
    format!(" {} / {total} ", sel.saturating_add(1).min(total))
}

/// 右下标签:`9 left · 48m → 15:32`。
///
/// - **剩余曲目**以在播曲为界(含在播曲本身):已播过的不计。在播曲已被摘出队列(悬空)
///   时整个队列都还没播,全部计入。
/// - **剩余时长**里当前曲只算未播部分(`duration − position`),后续整首;含未知时长的
///   曲目时加 `≥` 前缀——累计跳过了未知项,结果是下界而非实数。
/// - **`→ HH:MM`** 是「从现在起不间断播完」的预计钟点(`now + 剩余时长`),仅播放时显示;
///   暂停时隐去——不知道何时恢复,钟点会持续漂,隐去最诚实。剩余时长含未知项时一并隐去
///   (下界钟点会误导成「最早也要到那时」)。
///
/// 空队列 / 窄档给空串(不画右下)。
///
/// # Params:
///   - `ctx`: 应用状态(读队列 / 在播锚点 / 播放位置 / 播放态 / 本地钟点)
///   - `width`: 浮层内区宽,窄档退化为空
///
/// # Return:
///   剩余文本(两端各留一个空格);无内容时为空串。
pub(super) fn remaining_label(ctx: &AppState, width: u16) -> String {
    if ctx.player.queue.is_empty() || width < REMAINING_MIN_W {
        return String::new();
    }
    let from = ctx.queue_current_index().unwrap_or(0);
    let rest = ctx.player.queue.get(from..).unwrap_or(&[]);
    let (mut ms, unknown) = sum_durations(rest.iter().map(|s| s.duration_ms));
    // 在播曲(即 rest 首项)只剩未播部分:扣掉已播进度。悬空态无在播行,不扣。
    if ctx.player.cursor.is_attached()
        && let Some(dur) = rest.first().and_then(|s| s.duration_ms)
    {
        ms = ms.saturating_sub(dur.min(ctx.playback.position_ms));
    }
    let at_least = if unknown > 0 { "≥" } else { "" };
    let head = format!(" {} left · {at_least}{}", rest.len(), format_total(ms));
    // ends 钟点只在「播放中 + 时长精确」时给:暂停会漂,未知项让钟点变下界。
    if ctx.playback.playing && unknown == 0 {
        let now = ctx.now.get();
        let ends = now + chrono::Duration::milliseconds(i64::try_from(ms).unwrap_or(i64::MAX));
        format!("{head} → {} ", format_clock(now, ends))
    } else {
        format!("{head} ")
    }
}

#[cfg(test)]
mod tests {
    use mineral_test::song;

    use super::{position_label, remaining_label};
    use crate::runtime::state::AppState;

    /// 造一个队列:每项时长按 `durations` 给,在播锚点落 `current`,默认暂停、钟点固定。
    /// 固定 `now` 让 ends 快照/断言不随 wall-clock 漂。
    fn state_with(
        durations: &[Option<u64>],
        current: Option<usize>,
    ) -> color_eyre::Result<AppState> {
        use chrono::TimeZone;
        let mut state = AppState::test_default()?;
        let queue = durations
            .iter()
            .enumerate()
            .map(|(at, &ms)| {
                let mut s = song(&at.to_string());
                s.duration_ms = ms;
                s
            })
            .collect::<Vec<_>>();
        state.playback.track = current.and_then(|at| queue.get(at).cloned());
        state.player.cursor = mineral_protocol::PlayCursor::InQueue(current.unwrap_or(0));
        state.player.queue = queue;
        state.now.set(
            chrono::Local
                .with_ymd_and_hms(2026, 7, 20, 14, 44, 0)
                .single()
                .ok_or_else(|| color_eyre::eyre::eyre!("构造固定钟点失败"))?,
        );
        Ok(state)
    }

    /// 空队列:左下 `0 / 0`,右下空。
    #[test]
    fn empty_queue_shows_zero() -> color_eyre::Result<()> {
        let state = state_with(&[], /*current*/ None)?;
        assert_eq!(position_label(0, &state), " 0 / 0 ");
        assert_eq!(remaining_label(&state, /*width*/ 80), "");
        Ok(())
    }

    /// 暂停(默认)时:曲目数 · 时长,不显 ends 钟点。剩余以在播曲为界。
    #[test]
    fn paused_shows_count_and_duration_without_clock() -> color_eyre::Result<()> {
        let state = state_with(&[Some(60_000); 4], /*current*/ Some(2))?;
        assert_eq!(position_label(0, &state), " 1 / 4 ");
        assert_eq!(remaining_label(&state, /*width*/ 80), " 2 left · 2m ");
        Ok(())
    }

    /// 播放中:追加 `→ HH:MM` 预计播完钟点;当前曲只算未播部分(扣已播 position)。
    #[test]
    fn playing_appends_end_clock_and_deducts_elapsed() -> color_eyre::Result<()> {
        // 队列 4 首各 10 分钟,在播第 1 首已播 4 分钟 → 剩余 = 6 + 10 + 10 + 10 = 36 分钟。
        // now 14:44 + 36m = 15:20。
        let mut state = state_with(&[Some(600_000); 4], /*current*/ Some(0))?;
        state.playback.playing = true;
        state.playback.position_ms = 240_000; // 已播 4 分钟
        assert_eq!(
            remaining_label(&state, /*width*/ 80),
            " 4 left · 36m → 15:20 "
        );
        Ok(())
    }

    /// 含未知时长的曲目 → 时长带 `≥`(下界),且**不显** ends 钟点(下界钟点会误导)。
    #[test]
    fn unknown_durations_lower_bound_and_no_clock() -> color_eyre::Result<()> {
        let mut state = state_with(&[Some(60_000), None], /*current*/ Some(0))?;
        state.playback.playing = true;
        assert_eq!(remaining_label(&state, /*width*/ 80), " 2 left · ≥1m ");
        Ok(())
    }

    /// 窄档右下留空,左下位置照旧。
    #[test]
    fn narrow_width_drops_the_remaining_label() -> color_eyre::Result<()> {
        let state = state_with(&[Some(60_000); 2], /*current*/ Some(0))?;
        assert_eq!(position_label(1, &state), " 2 / 2 ");
        assert_eq!(remaining_label(&state, /*width*/ 20), "");
        Ok(())
    }
}
