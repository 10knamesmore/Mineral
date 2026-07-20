//! 导航:按 [`PlayMode`] 推「下一首 / 上一首」,以及进 / 退 shuffle 边界的洗牌 / 还原。
//!
//! 全部是面向 [`State`] 的自由函数(shuffle 三件改 State,其余只读)。

use mineral_model::Song;
use mineral_protocol::{PlayCursor, PlayMode};
use rand::seq::SliceRandom;

use crate::state::State;

/// 队列硬上限:任何入队路径都不得让 `queue` 长度超过此值。
///
/// 满时 [`append`] / [`insert_next`] 拒绝入队,[`super::PlayerCore::set_queue`] 截断到此长度。
/// 取 9999 与序号显示上限一致(0-based 下标故最大 9998,四位数封顶)。
pub(crate) const QUEUE_CAP: usize = 9999;

/// 队列是否已达 [`QUEUE_CAP`](满则拒绝再入队)。
fn at_capacity(st: &State) -> bool {
    st.queue.len() >= QUEUE_CAP
}

/// 按 [`PlayMode`] 计算「下一首」的**下标**:Sequential 到尾返回 None,Repeat/Shuffle 环回 0,RepeatOne 原地。
///
/// 推进以**下标**为真相,不经歌曲身份——队列含重复曲时,按身份 first-match 定位会把位置吸附到
/// 首个副本,两首交替的重复曲会互相指回对方,造成无限循环跳不出去。
///
/// 被 hook 否决的下标(`prefetch_vetoed`,本窗口内「这次不播」)一律越过——预测
/// (`next_in_queue`)与边界推进(`advance_next`)都经这里,否决语义单点生效。
/// RepeatOne 的循环曲被否决时顺出到下一首非否决曲(原地重复必然再失败)。
///
/// # Params:
///   - `st`: 播放状态(读 queue / queue_sel / play_mode / prefetch_vetoed)
///
/// # Return:
///   下一首的下标;无(队列空 / Sequential 到尾 / 候选全被否决)则 `None`。
pub(crate) fn next_index(st: &State) -> Option<usize> {
    let len = st.queue.len();
    if len == 0 {
        return None;
    }
    let not_vetoed = |idx: &usize| !st.prefetch_vetoed.contains(idx);
    // 悬空:当前曲已被摘出队列、不占下标,接续点本身就是「下一首」——再 +1 会白白跳过
    // 一首。RepeatOne 在此让位:把歌从队列删掉的意图就是不再听它,继续循环直接违背该意图。
    if let PlayCursor::Detached { resume_at } = st.cursor {
        return match st.play_mode {
            PlayMode::Sequential => (resume_at..len).find(not_vetoed),
            PlayMode::RepeatAll | PlayMode::Shuffle | PlayMode::RepeatOne => (0..len)
                .map(|offset| (resume_at + offset) % len)
                .find(not_vetoed),
        };
    }
    let cur = st.cursor.anchor().min(len - 1);
    if st.play_mode == PlayMode::RepeatOne && not_vetoed(&cur) {
        return Some(cur);
    }
    match st.play_mode {
        PlayMode::Sequential => ((cur + 1)..len).find(not_vetoed),
        // 环回模式最多扫一整圈(含转回 cur 自身:其余全被否决时当前曲仍可重播);
        // RepeatOne 走到这里 = 循环曲已被否决,按环回语义顺出。
        PlayMode::RepeatAll | PlayMode::Shuffle | PlayMode::RepeatOne => (1..=len)
            .map(|offset| (cur + offset) % len)
            .find(not_vetoed),
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
    // 悬空:当前曲不占下标,「上一首」从接续点往前退一格。
    if let PlayCursor::Detached { resume_at } = st.cursor {
        return match st.play_mode {
            PlayMode::Sequential => (resume_at > 0).then(|| resume_at - 1),
            PlayMode::RepeatAll | PlayMode::Shuffle => Some((resume_at + len - 1) % len),
            PlayMode::RepeatOne => Some(resume_at.min(len - 1)),
        };
    }
    let cur = st.cursor.anchor().min(len - 1);
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
    st.cursor = PlayCursor::InQueue(idx);
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
    st.cursor = PlayCursor::InQueue(idx);
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

/// 队列重排后重新定位游标的锚点歌——附着态锚当前曲,悬空态锚「接续曲」。
///
/// 悬空时当前曲已不在队列里,唯一有意义的锚是它播完后要接的那首:洗牌只该打乱顺序,
/// 不该改变这个接续关系。返回 `None` 表示无锚(接续点已越过队尾 = 播完即停)。
fn reanchor_id(st: &State) -> Option<mineral_model::SongId> {
    match st.cursor {
        PlayCursor::InQueue(_) => st.current_song.as_ref().map(|s| s.id.clone()),
        PlayCursor::Detached { resume_at } => st.queue.get(resume_at).map(|s| s.id.clone()),
    }
}

/// 重排后把锚点歌挪到 0 位并据此重设游标(附着态当前曲在 0,悬空态接续点在 0)。
/// 无锚(悬空且播完即停)时保持「播完即停」:接续点取新队列长度。
fn restore_anchor(st: &mut State, anchor: Option<mineral_model::SongId>, swap_to_front: bool) {
    let found = anchor.and_then(|id| st.queue.iter().position(|s| s.id == id));
    if swap_to_front && let Some(pos) = found {
        st.queue.swap(0, pos);
    }
    let at = if swap_to_front {
        found.map(|_| 0)
    } else {
        found
    };
    st.cursor = match st.cursor {
        PlayCursor::InQueue(_) => PlayCursor::InQueue(at.unwrap_or(0)),
        PlayCursor::Detached { .. } => PlayCursor::Detached {
            resume_at: at.unwrap_or(st.queue.len()),
        },
    };
}

/// 进入 shuffle:存原序到 `original_queue`,洗牌后把锚点歌挪到 0 位。
pub(crate) fn enter_shuffle(st: &mut State) {
    if st.queue.is_empty() {
        return;
    }
    let original = st.queue.clone();
    let anchor = reanchor_id(st);
    st.queue.shuffle(&mut rand::rng());
    restore_anchor(st, anchor, /*swap_to_front*/ true);
    st.original_queue = Some(original);
    st.bump_queue();
}

/// 退出 shuffle:从 `original_queue` 还原顺序,游标重新定位到锚点歌。
pub(crate) fn exit_shuffle(st: &mut State) {
    let Some(original) = st.original_queue.take() else {
        return;
    };
    let anchor = reanchor_id(st);
    st.queue = original;
    restore_anchor(st, anchor, /*swap_to_front*/ false);
    st.bump_queue();
}

/// 插播:`song` 插到当前曲之后;shuffle 模式下同步插入 `original_queue` 的
/// 当前曲之后(退出 shuffle 时位置仍合理)。不动 `queue_sel` 与当前曲。
pub(crate) fn insert_next(st: &mut State, song: Song) {
    if at_capacity(st) {
        mineral_log::debug!(target: "player", cap = QUEUE_CAP, "queue at capacity, insert dropped");
        return;
    }
    let orig_at = original_insert_pos(st);
    let pos = after_current_pos(st);
    st.queue.insert(pos, song.clone());
    if let Some(orig) = st.original_queue.as_mut() {
        orig.insert(orig_at.min(orig.len()), song);
    }
    st.bump_queue();
}

/// 「当前曲之后」在 `queue` 中的插入下标。
///
/// 悬空时当前曲已不占下标,接续点本身就是「紧随当前曲之后」的位置。
pub(crate) fn after_current_pos(st: &State) -> usize {
    let at = match st.cursor {
        PlayCursor::InQueue(cur) => cur + 1,
        PlayCursor::Detached { resume_at } => resume_at,
    };
    at.min(st.queue.len())
}

/// 「当前曲之后」在 `original_queue` 中的插入下标(非 shuffle 时该表不存在,值被忽略)。
///
/// 附着态锚当前曲、插其后;悬空态锚接续曲、插其前——两者都表达「紧随当前曲」。
fn original_insert_pos(st: &State) -> usize {
    let Some(orig) = st.original_queue.as_ref() else {
        return 0;
    };
    match st.cursor {
        PlayCursor::InQueue(_) => st
            .current_song
            .as_ref()
            .and_then(|cur| orig.iter().position(|s| s.id == cur.id))
            .map_or(orig.len(), |i| i + 1),
        PlayCursor::Detached { resume_at } => st
            .queue
            .get(resume_at)
            .and_then(|next| orig.iter().position(|s| s.id == next.id))
            .unwrap_or(orig.len()),
    }
}

/// 追加到队列末尾;shuffle 模式下同步追加 `original_queue`。
pub(crate) fn append(st: &mut State, song: Song) {
    if at_capacity(st) {
        mineral_log::debug!(target: "player", cap = QUEUE_CAP, "queue at capacity, append dropped");
        return;
    }
    st.queue.push(song.clone());
    if let Some(orig) = st.original_queue.as_mut() {
        orig.push(song);
    }
    st.bump_queue();
}

#[cfg(test)]
mod tests {
    use mineral_protocol::{PlayCursor, PlayMode};
    use mineral_test::song;

    use super::{QUEUE_CAP, advance_next, append, insert_next, next_index};
    use crate::state::State;

    /// 造一个 3 曲队列(a/b/c),当前在 a,指定模式。
    fn state_with_mode(mode: PlayMode) -> State {
        let mut st = State::empty();
        st.queue = vec![song("a"), song("b"), song("c")];
        st.cursor = PlayCursor::InQueue(0);
        st.play_mode = mode;
        st
    }

    /// 造一个「当前曲已被摘出队列」的悬空态:队列剩 b/c,a 仍在响,播完应接 b。
    fn detached_state(mode: PlayMode) -> State {
        let mut st = State::empty();
        st.queue = vec![song("b"), song("c")];
        st.current_song = Some(song("a"));
        st.cursor = PlayCursor::Detached { resume_at: 0 };
        st.play_mode = mode;
        st
    }

    /// 悬空时接续点就是「下一首」本身,不再 +1——否则会白白跳过一首。
    #[test]
    fn detached_resumes_at_successor_itself() {
        for mode in [PlayMode::Sequential, PlayMode::RepeatAll, PlayMode::Shuffle] {
            let st = detached_state(mode);
            assert_eq!(next_index(&st), Some(0), "{mode:?} 悬空时应落在接续点本身");
        }
    }

    /// 悬空 + RepeatOne:当前曲已被摘出队列,单曲循环让位于接续点。
    /// 把歌从队列删掉的意图就是不再听它,继续循环会直接违背该意图。
    #[test]
    fn detached_repeat_one_yields_to_successor() {
        let st = detached_state(PlayMode::RepeatOne);
        assert_eq!(next_index(&st), Some(0), "被摘出的当前曲不该继续循环");
    }

    /// 悬空时接续点已越过队尾(删的是最后一首)→ 播完即停。
    #[test]
    fn detached_past_tail_stops() {
        let mut st = detached_state(PlayMode::Sequential);
        st.queue = vec![song("b")];
        st.cursor = PlayCursor::Detached { resume_at: 1 };
        assert_eq!(next_index(&st), None, "接续点越过队尾应停止");
    }

    /// 否决下一首(b):Sequential/RepeatAll 的预测都应越过 b 落到 c。
    #[test]
    fn vetoed_next_is_skipped() {
        for mode in [PlayMode::Sequential, PlayMode::RepeatAll] {
            let mut st = state_with_mode(mode);
            st.prefetch_vetoed = vec![1];
            assert_eq!(next_index(&st), Some(2), "{mode:?} 应越过被否决的 b 落到 c");
        }
    }

    /// 连续否决(b、c 全被否决):Sequential 无候选 → None;RepeatAll 环回落到 a(重播当前)。
    #[test]
    fn all_following_vetoed() {
        let mut st = state_with_mode(PlayMode::Sequential);
        st.prefetch_vetoed = vec![1, 2];
        assert_eq!(next_index(&st), None, "顺序模式候选耗尽应无下一首");

        let mut st = state_with_mode(PlayMode::RepeatAll);
        st.prefetch_vetoed = vec![1, 2];
        assert_eq!(
            next_index(&st),
            Some(0),
            "环回模式转一圈落回当前曲(唯一非否决候选)"
        );
    }

    /// RepeatOne:循环曲未被否决 → 原地;被否决 → 顺出到下一首非否决曲。
    #[test]
    fn repeat_one_vetoed_falls_through() {
        let mut st = state_with_mode(PlayMode::RepeatOne);
        assert_eq!(next_index(&st), Some(0), "未否决时原地循环");
        st.prefetch_vetoed = vec![0];
        assert_eq!(next_index(&st), Some(1), "循环曲被否决应顺出到 b");
    }

    /// 边界推进(advance_next)与预测共用同一越过逻辑:queue_sel 直接跳到 c。
    #[test]
    fn advance_next_honors_veto() -> color_eyre::Result<()> {
        let mut st = state_with_mode(PlayMode::Sequential);
        st.prefetch_vetoed = vec![1];
        let next = advance_next(&mut st).ok_or_else(|| color_eyre::eyre::eyre!("应有下一首"))?;
        assert_eq!(next.id, song("c").id, "Fallback 推进应越过被否决的 b");
        assert_eq!(st.cursor, PlayCursor::InQueue(2));
        Ok(())
    }

    /// 队列硬上限 [`QUEUE_CAP`]:满队列 append / insert_next 均 no-op,长度不越界。
    #[test]
    fn queue_capacity_is_capped() {
        let mut st = State::empty();
        st.queue = (0..QUEUE_CAP).map(|i| song(&i.to_string())).collect();
        assert_eq!(st.queue.len(), QUEUE_CAP);
        append(&mut st, song("overflow"));
        assert_eq!(st.queue.len(), QUEUE_CAP, "满队列 append 应被拒");
        insert_next(&mut st, song("overflow2"));
        assert_eq!(st.queue.len(), QUEUE_CAP, "满队列 insert_next 应被拒");
    }

    /// 未满时 append / insert_next 照常入队(守卫不误伤正常路径)。
    #[test]
    fn under_capacity_still_enqueues() {
        let mut st = State::empty();
        append(&mut st, song("a"));
        insert_next(&mut st, song("b"));
        assert_eq!(st.queue.len(), 2, "未满时正常入队");
    }

    /// 无否决时行为与既有语义一致(回归保护)。
    #[test]
    fn no_veto_keeps_existing_semantics() {
        assert_eq!(next_index(&state_with_mode(PlayMode::Sequential)), Some(1));
        assert_eq!(next_index(&state_with_mode(PlayMode::RepeatAll)), Some(1));
        assert_eq!(next_index(&state_with_mode(PlayMode::RepeatOne)), Some(0));
        let mut tail = state_with_mode(PlayMode::Sequential);
        tail.cursor = PlayCursor::InQueue(2);
        assert_eq!(next_index(&tail), None, "顺序到尾无下一首");
        let mut wrap = state_with_mode(PlayMode::RepeatAll);
        wrap.cursor = PlayCursor::InQueue(2);
        assert_eq!(next_index(&wrap), Some(0), "环回到 0");
    }
}
