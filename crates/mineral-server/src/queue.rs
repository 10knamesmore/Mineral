//! 队列计算:按 [`PlayMode`] 推「下一首 / 上一首」、PlayMode 的稳定字符串化,
//! 以及进 / 退 shuffle 边界的洗牌 / 还原。
//!
//! 全部是面向 [`State`] 的自由函数(shuffle 三件改 State,其余只读)。

use mineral_model::Song;
use mineral_protocol::PlayMode;
use rand::seq::SliceRandom;

use crate::state::State;

/// [`PlayMode`] → 稳定字符串(如 `"Sequential"`),启动恢复经 `PlayMode::from_name` 解析回来。
///
/// # Params:
///   - `mode`: 播放模式
///
/// # Return:
///   稳定名字符串(与历史 Debug 落库值一致)。
pub(crate) fn play_mode_str(mode: PlayMode) -> String {
    mode.name().to_owned()
}

/// 按 [`PlayMode`] 计算「下一首」的**下标**:Sequential 到尾返回 None,Repeat/Shuffle 环回 0,RepeatOne 原地。
///
/// 推进以**下标**为真相,不经歌曲身份——队列含重复曲时,按身份 first-match 定位会把位置吸附到
/// 首个副本,两首交替的重复曲会互相指回对方,造成无限循环跳不出去。
///
/// # Params:
///   - `st`: 播放状态(读 queue / queue_sel / play_mode)
///
/// # Return:
///   下一首的下标;无则 `None`。
pub(crate) fn next_index(st: &State) -> Option<usize> {
    let len = st.queue.len();
    if len == 0 {
        return None;
    }
    let cur = st.queue_sel.min(len - 1);
    match st.play_mode {
        PlayMode::Sequential => (cur + 1 < len).then(|| cur + 1),
        PlayMode::RepeatAll | PlayMode::Shuffle => Some((cur + 1) % len),
        PlayMode::RepeatOne => Some(cur),
    }
}

/// 按 [`PlayMode`] 计算「上一首」的**下标**:Sequential 在 0 时返回 None,Repeat/Shuffle 环回末尾,RepeatOne 原地。
/// 同 [`next_index`],以下标为真相、不经歌曲身份。
///
/// # Params:
///   - `st`: 播放状态(读 queue / queue_sel / play_mode)
///
/// # Return:
///   上一首的下标;无则 `None`。
pub(crate) fn prev_index(st: &State) -> Option<usize> {
    let len = st.queue.len();
    if len == 0 {
        return None;
    }
    let cur = st.queue_sel.min(len - 1);
    match st.play_mode {
        PlayMode::Sequential => (cur > 0).then(|| cur - 1),
        PlayMode::RepeatAll | PlayMode::Shuffle => Some((cur + len - 1) % len),
        PlayMode::RepeatOne => Some(cur),
    }
}

/// 只读取「下一首」歌(不动 `queue_sel`):gapless 预排探下一曲用,此刻还没真切歌。
///
/// # Params:
///   - `st`: 播放状态
///
/// # Return:
///   下一首歌;无则 `None`。
pub(crate) fn next_in_queue(st: &State) -> Option<Song> {
    next_index(st).and_then(|i| st.queue.get(i).cloned())
}

/// 顺序推进到「下一首」:把 `queue_sel` 钉到 [`next_index`] 的下标并返回该曲。
/// 切歌入口(`n` 键 / gapless 兜底)走这条,确保位置按下标单向前进。
///
/// # Params:
///   - `st`: 播放状态(写 `queue_sel`)
///
/// # Return:
///   推进到的歌;到尾(Sequential)无下一首则不动、返回 `None`。
pub(crate) fn advance_next(st: &mut State) -> Option<Song> {
    let idx = next_index(st)?;
    st.queue_sel = idx;
    st.queue.get(idx).cloned()
}

/// 顺序后退到「上一首」:把 `queue_sel` 钉到 [`prev_index`] 的下标并返回该曲。
///
/// # Params:
///   - `st`: 播放状态(写 `queue_sel`)
///
/// # Return:
///   后退到的歌;在首位(Sequential)无上一首则不动、返回 `None`。
pub(crate) fn advance_prev(st: &mut State) -> Option<Song> {
    let idx = prev_index(st)?;
    st.queue_sel = idx;
    st.queue.get(idx).cloned()
}

/// 设置 PlayMode,并在进 / 退 Shuffle 边界处洗牌或还原 queue。模式不变则 no-op。
///
/// # Params:
///   - `st`: 播放状态(写 play_mode,边界处改 queue)
///   - `new`: 目标模式
pub(crate) fn apply_play_mode(st: &mut State, new: PlayMode) {
    let old = st.play_mode;
    if old == new {
        return;
    }
    mineral_log::info!(target: "player", old = ?old, new = ?new, "play mode changed");
    st.play_mode = new;
    match (old == PlayMode::Shuffle, new == PlayMode::Shuffle) {
        (false, true) => enter_shuffle(st),
        (true, false) => exit_shuffle(st),
        _ => {}
    }
}

/// 进入 shuffle:存原序到 `original_queue`,洗牌后把当前歌挪到 0 位、`queue_sel = 0`。
pub(crate) fn enter_shuffle(st: &mut State) {
    if st.queue.is_empty() {
        return;
    }
    let original = st.queue.clone();
    let cur_id = st.current_song.as_ref().map(|t| t.id.clone());
    st.queue.shuffle(&mut rand::rng());
    if let Some(id) = cur_id
        && let Some(pos) = st.queue.iter().position(|s| s.id == id)
    {
        st.queue.swap(0, pos);
    }
    st.queue_sel = 0;
    st.original_queue = Some(original);
    st.bump_queue();
}

/// 退出 shuffle:从 `original_queue` 还原顺序,`queue_sel` 重新定位到当前歌。
pub(crate) fn exit_shuffle(st: &mut State) {
    let Some(original) = st.original_queue.take() else {
        return;
    };
    let cur_id = st.current_song.as_ref().map(|t| t.id.clone());
    st.queue = original;
    st.queue_sel = cur_id
        .and_then(|id| st.queue.iter().position(|s| s.id == id))
        .unwrap_or(0);
    st.bump_queue();
}

/// 插播:`song` 插到当前曲之后;shuffle 模式下同步插入 `original_queue` 的
/// 当前曲之后(退出 shuffle 时位置仍合理)。不动 `queue_sel` 与当前曲。
pub(crate) fn insert_next(st: &mut State, song: Song) {
    let cur_id = st.current_song.as_ref().map(|s| s.id.clone());
    let pos = (st.queue_sel + 1).min(st.queue.len());
    st.queue.insert(pos, song.clone());
    if let Some(orig) = st.original_queue.as_mut() {
        let at = cur_id
            .and_then(|id| orig.iter().position(|s| s.id == id))
            .map_or(orig.len(), |i| i + 1);
        orig.insert(at, song);
    }
    st.bump_queue();
}

/// 追加到队列末尾;shuffle 模式下同步追加 `original_queue`。
pub(crate) fn append(st: &mut State, song: Song) {
    st.queue.push(song.clone());
    if let Some(orig) = st.original_queue.as_mut() {
        orig.push(song);
    }
    st.bump_queue();
}
