//! 队列计算:按 [`PlayMode`] 推「下一首 / 上一首」,以及 PlayMode 的稳定字符串化。
//!
//! 都是只读 [`State`] 的纯函数;进 / 退 shuffle 的洗牌还原(改 State)留在 player 内。

use mineral_model::Song;
use mineral_protocol::PlayMode;

use crate::state::State;

/// [`PlayMode`] → 稳定字符串(Debug 名,如 `"Sequential"`)。本轮不 parse 回来。
///
/// # Params:
///   - `mode`: 播放模式
///
/// # Return:
///   稳定的 Debug 名字符串。
pub(crate) fn play_mode_str(mode: PlayMode) -> String {
    format!("{mode:?}")
}

/// 按 [`PlayMode`] 计算「下一首」:Sequential 到尾返回 None,Repeat/Shuffle 环回 0,RepeatOne 原地。
///
/// # Params:
///   - `st`: 播放状态(读 queue / queue_sel / play_mode)
///
/// # Return:
///   下一首歌;无则 `None`。
pub(crate) fn next_in_queue(st: &State) -> Option<Song> {
    let len = st.queue.len();
    if len == 0 {
        return None;
    }
    let cur = st.queue_sel.min(len - 1);
    match st.play_mode {
        PlayMode::Sequential => st.queue.get(cur + 1).cloned(),
        PlayMode::RepeatAll | PlayMode::Shuffle => st.queue.get((cur + 1) % len).cloned(),
        PlayMode::RepeatOne => st.queue.get(cur).cloned(),
    }
}

/// 按 [`PlayMode`] 计算「上一首」:Sequential 在 0 时返回 None,Repeat/Shuffle 环回末尾,RepeatOne 原地。
///
/// # Params:
///   - `st`: 播放状态(读 queue / queue_sel / play_mode)
///
/// # Return:
///   上一首歌;无则 `None`。
pub(crate) fn prev_in_queue(st: &State) -> Option<Song> {
    let len = st.queue.len();
    if len == 0 {
        return None;
    }
    let cur = st.queue_sel.min(len - 1);
    match st.play_mode {
        PlayMode::Sequential => {
            if cur == 0 {
                None
            } else {
                st.queue.get(cur - 1).cloned()
            }
        }
        PlayMode::RepeatAll | PlayMode::Shuffle => st.queue.get((cur + len - 1) % len).cloned(),
        PlayMode::RepeatOne => st.queue.get(cur).cloned(),
    }
}
