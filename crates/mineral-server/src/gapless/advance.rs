//! 曲终推进裁决与已预排媒体的状态轮转。

use mineral_model::SongId;
use mineral_protocol::{AdvanceKind, PlayCursor};

use crate::queue::next_index;
use crate::state::State;

/// 曲终(finished_seq 前进)时服务端该走的推进动作。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Advance {
    /// 无曲终事件,不动。
    None,

    /// 引擎已无缝轮转(仍在出声)且有已预排曲:扶正记账,不重播。
    Adopt,

    /// 曲终但未无缝(队尾静音 / 未预排):走 `play_song` 兜底(有间隙)。
    Fallback,
}

/// 据「finished_seq 是否前进 + 是否仍在出声 + 是否有已预排曲」判定推进动作。
///
/// 仍在出声 ⇒ 引擎做了无缝轮转(next 已 append 接上);停了 ⇒ 队尾静音(next 未就绪),
/// 此时即便服务端记着 queued 也要兜底重播,否则会把记账扶正到一首没在响的歌。
///
/// # Params:
///   - `finished_advanced`: snapshot 的 `track_finished_seq` 是否比上次见到的大
///   - `playing`: 当前是否仍在出声
///   - `has_queued`: 服务端是否记着一首已预排曲
///
/// # Return:
///   推进动作。
pub(crate) fn decide_advance(finished_advanced: bool, playing: bool, has_queued: bool) -> Advance {
    if !finished_advanced {
        return Advance::None;
    }
    if playing && has_queued {
        Advance::Adopt
    } else {
        Advance::Fallback
    }
}

/// 无缝边界已由引擎完成轮转(下一曲正在播),服务端据此把「已预排」扶正为「当前」:
/// current=queued、queue_sel 推进到它在队列的位置、resolved/origin 轮转、
/// 歌词与预拉状态复位。
///
/// # Params:
///   - `st`: 播放状态(原地轮转)
///
/// # Return:
///   被顶替的旧当前歌 id(供打点);无已预排曲则 `None` 且不改状态。
pub(crate) fn adopt_queued(st: &mut State) -> Option<SongId> {
    let (slot, queued) = st.prefetch.take_armed()?;
    let old_id = st.current_song.as_ref().map(|s| s.id.clone());
    if let Some(current) = st.current_slot.take() {
        current.cancel();
    }
    // 游标此刻仍指旧当前曲(或其接续点);预排曲就是当时 next_index 算出的那一首(队列一变
    // 即作废预排),故按下标推进,**不**按 queued.song 身份 first-match——重复曲会把下标
    // 吸附到首个副本。轮转到队列内的曲即离开悬空态。
    if let Some(idx) = next_index(st) {
        st.cursor = PlayCursor::InQueue(idx);
    }
    // 无缝接续也是一次「下一首」推进:落账进入档位,与 [`crate::queue::advance_next`]
    // 同一条语义(client 全屏切歌转场据此定向)。
    st.advance = Some((queued.song.id.clone(), AdvanceKind::Next));
    st.current_song = Some(queued.song);
    st.current_slot = Some(slot);
    st.media_info = Some(queued.media_info);
    st.direct_media = queued.direct_media;
    st.play_origin = Some(queued.origin);
    st.current_lyrics = None;
    st.current_lyrics_song_id = None;
    // 边界消费:本窗口的否决已完成使命(预测/推进都越过了被否决曲),清空。
    st.prefetch_vetoed.clear();
    st.bump_current();
    old_id
}

#[cfg(test)]
mod tests {
    use mineral_model::{BitRate, PlaybackMediaInfo};
    use mineral_protocol::{AdvanceKind, PlayCursor, PlaybackOrigin};
    use mineral_test::song;

    use super::{Advance, adopt_queued, decide_advance};
    use crate::gapless::Queued;
    use crate::playback_instance::PlaybackSlot;
    use crate::state::State;

    /// decide_advance:无曲终 → None;无缝(仍出声 + 有预排)→ Adopt;否则 → Fallback。
    #[test]
    fn decide_advance_branches() {
        assert_eq!(
            decide_advance(
                /*finished*/ false, /*playing*/ true, /*queued*/ true
            ),
            Advance::None,
            "无曲终不动"
        );
        assert_eq!(
            decide_advance(
                /*finished*/ true, /*playing*/ true, /*queued*/ true
            ),
            Advance::Adopt,
            "仍出声 + 有预排 → 无缝采纳"
        );
        assert_eq!(
            decide_advance(
                /*finished*/ true, /*playing*/ false, /*queued*/ true
            ),
            Advance::Fallback,
            "停了(队尾静音)即便有预排也要兜底"
        );
        assert_eq!(
            decide_advance(
                /*finished*/ true, /*playing*/ true, /*queued*/ false
            ),
            Advance::Fallback,
            "无预排 → 兜底"
        );
    }

    /// adopt_queued:queued 顶成 current、queue_sel 定位、origin 轮转、预拉状态复位,返回旧 id。
    #[test]
    fn adopt_rotates_queued_into_current() {
        let mut st = State::empty();
        st.queue = vec![song("a"), song("b")];
        st.cursor = PlayCursor::InQueue(0);
        st.current_song = Some(song("a"));
        st.current_slot = Some(PlaybackSlot::new(song("a").id));
        let next_slot = PlaybackSlot::new(song("b").id);
        st.prefetch.arm(
            next_slot,
            Queued {
                song: song("b"),
                media_info: PlaybackMediaInfo {
                    song_id: song("b").id,
                    bitrate_bps: None,
                    quality: BitRate::Higher,
                    size: None,
                    format: None,
                    bit_depth: None,
                    substituted: false,
                },
                direct_media: None,
                origin: PlaybackOrigin::Remote,
            },
        );

        let old = adopt_queued(&mut st);
        assert_eq!(old, Some(song("a").id), "应返回被顶替的旧当前歌 id");
        assert_eq!(
            st.current_song.as_ref().map(|s| s.id.clone()),
            Some(song("b").id),
            "current 应变成 queued"
        );
        assert_eq!(st.cursor, PlayCursor::InQueue(1), "游标应定位到 b");
        assert_eq!(st.play_origin, Some(PlaybackOrigin::Remote));
        assert_eq!(
            st.advance,
            Some((song("b").id, AdvanceKind::Next)),
            "无缝接续也是一次下一首推进(client 转场据此定向)"
        );
        assert!(!st.prefetch.is_armed(), "armed prefetch 应被取走");
    }

    /// adopt_queued:无已预排曲时返回 None 且不动当前歌。
    #[test]
    fn adopt_without_queued_is_noop() {
        let mut st = State::empty();
        st.current_song = Some(song("a"));
        assert!(adopt_queued(&mut st).is_none());
        assert_eq!(
            st.current_song.as_ref().map(|s| s.id.clone()),
            Some(song("a").id),
            "无 queued 不应改动当前歌"
        );
    }
}
