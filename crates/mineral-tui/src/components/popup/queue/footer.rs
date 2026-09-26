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
