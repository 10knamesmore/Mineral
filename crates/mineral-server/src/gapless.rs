//! 服务端 gapless 编排:把「已预排的下一曲」状态在无缝边界处扶正为「当前曲」,
//! 以及预排 / 并发 capture 收割相关的纯状态变换。
//!
//! 引擎([`mineral_audio`])在当前曲自然耗尽时已把下一曲零静音接上;服务端这边只需在
//! 边界处把记账状态轮转过来(current=queued、queue_sel 推进、resolved/origin/capturing
//! 轮转、歌词与预拉复位),**不**重新 `play_song`(音频没有中断)。

mod state;

use mineral_model::{Song, SongId};
use mineral_playback::{DirectMedia, OpenedMedia};
use mineral_protocol::{PlayCursor, PlaybackOrigin};
use mineral_task::{ChannelFetchKind, Priority, TaskKind};

use crate::download::Capturing;
use crate::playback_instance::PlaybackSlot;
use crate::player::PlayerCore;
use crate::queue::{advance_next, next_in_queue, next_index};
use crate::state::State;

/// 记一次预取裁决(prefetches;系统域,无 actor)。装填 / 否决 / 改写 / 失败各站点调。
///
/// # Params:
///   - `song`: 预取的下一曲
///   - `source`: 预取来源(本地 / 远端 / 单曲循环)
///   - `resolution`: 裁决(装填 / 否决 / 改写 / 失败)
pub(crate) fn record_prefetch(
    player: &PlayerCore,
    song: SongId,
    source: mineral_stats::PrefetchSource,
    resolution: mineral_stats::PrefetchResolution,
) {
    player.inner.stats.event(mineral_stats::StatsEvent::System(
        mineral_stats::SystemEvent::Prefetch {
            song,
            source,
            resolution,
        },
    ));
}

pub(crate) use state::{PrefetchState, Queued};

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
/// current=queued、queue_sel 推进到它在队列的位置、resolved/origin/capturing 轮转、
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
    st.current_song = Some(queued.song);
    st.current_slot = Some(slot);
    st.media_info = Some(queued.media_info);
    st.direct_media = queued.direct_media;
    st.play_origin = Some(queued.origin);
    st.capturing = queued.capturing;
    st.current_lyrics = None;
    st.current_lyrics_song_id = None;
    // 边界消费:本窗口的否决已完成使命(预测/推进都越过了被否决曲),清空。
    st.prefetch_vetoed.clear();
    st.bump_current();
    old_id
}

/// 预排窗口是否已开(距曲终 ≤ `window_ms`)。
///
/// 总时长优先取 decoder 实测(顶换流的元数据描述的是原源音频,实测才是真口径);
/// 分片容器(fMP4)以流式打开时 decoder 探不出总时长,回落当前曲元数据——不回落的话
/// 这类源的预排窗口永远不开,gapless 整体失效。两口径都未知 → 不开窗。
///
/// # Params:
///   - `engine_duration_ms`: decoder 实测总时长(探不出为 `None`)
///   - `metadata_duration_ms`: 当前曲元数据时长(未知为 `None`)
///   - `position_ms`: 当前播放位置(ms)
///   - `window_ms`: 预排提前量(配置 `daemon.gapless_prefetch_ms`)
///
/// # Return:
///   窗口是否已开。
fn prefetch_window_open(
    engine_duration_ms: Option<u64>,
    metadata_duration_ms: Option<u64>,
    position_ms: u64,
    window_ms: u64,
) -> bool {
    let Some(duration_ms) = engine_duration_ms.or(metadata_duration_ms) else {
        return false;
    };
    duration_ms.saturating_sub(position_ms) <= window_ms
}

/// Starts one independent provider/local prefetch attempt inside the gapless window.
pub(crate) fn check_prefetch(player: &PlayerCore) {
    let snap = player.audio_snapshot();
    let metadata_duration_ms =
        player.with_state(|st| st.current_song.as_ref().and_then(|s| s.duration_ms));
    if !prefetch_window_open(
        snap.duration_ms,
        metadata_duration_ms,
        snap.position_ms,
        player.gapless_prefetch_ms(),
    ) {
        return;
    }
    let next = player.with_state(|st| {
        st.current_song.as_ref()?;
        if st.prefetch.is_armed() {
            return None;
        }
        let next = next_in_queue(st);
        if let Some(n) = next.as_ref()
            && st
                .prefetch
                .song_id()
                .is_some_and(|song_id| *song_id == n.id)
        {
            return None;
        }
        next
    });
    let Some(next) = next else {
        return;
    };
    let slot = PlaybackSlot::new(next.id.clone());
    player.with_state(|st| {
        st.prefetch.replace_opening(slot.clone());
    });
    crate::playback::start_prefetch(player, next, slot);
}

/// Arms already-opened next media and records its promotion facts.
///
/// # Params:
///   - `player`: Playback owner.
///   - `song`: Prefetched song snapshot.
///   - `slot`: Still-active prefetch ownership slot.
///   - `opened`: Already-opened decoder input.
///   - `direct`: Optional direct capability.
///   - `origin`: Cache, download-library, or provider provenance.
///   - `cacheable`: Whether post-preparation bytes may enter the cache.
pub(crate) fn arm_opened(
    player: &PlayerCore,
    song: Song,
    slot: &PlaybackSlot,
    opened: OpenedMedia,
    direct: Option<DirectMedia>,
    origin: PlaybackOrigin,
    cacheable: bool,
) {
    let info = opened.info().clone();
    let capture = cacheable
        .then(|| {
            player
                .media_cache()
                .capture_path(&song.id, player.playback_quality())
        })
        .flatten();
    let armed = player.with_state(|st| {
        let Some(active) = st.prefetch.take_opening(slot.instance_id, &song.id) else {
            return false;
        };
        let capturing = match capture {
            Some(path) => {
                player.audio().append_next_capturing(opened, path.clone());
                Some(Capturing {
                    song: song.clone(),
                    quality: player.playback_quality(),
                    format: info.format.clone(),
                    path,
                })
            }
            None => {
                player.audio().append_next(opened);
                None
            }
        };
        st.prefetch.arm(
            active,
            Queued {
                song: song.clone(),
                media_info: info.clone(),
                direct_media: direct,
                origin,
                capturing,
            },
        );
        true
    });
    if !armed {
        return;
    }
    let source = match origin {
        PlaybackOrigin::Cache | PlaybackOrigin::Download => mineral_stats::PrefetchSource::Local,
        PlaybackOrigin::Remote => mineral_stats::PrefetchSource::Remote,
    };
    record_prefetch(
        player,
        song.id,
        source,
        mineral_stats::PrefetchResolution::Armed,
    );
}

/// 收割已下完的 capture 进缓存:当前曲(`download_complete`)+ 已预排曲(`next_download_complete`),
/// 两路并发各取各的 [`Capturing`](不同曲不同临时路径,结构上不撞)。
pub(crate) fn check_harvest(player: &PlayerCore) {
    let snap = player.audio_snapshot();
    if snap.download_complete {
        let cap = player.with_state(|st| st.capturing.take());
        if let Some(cap) = cap {
            crate::download::spawn_harvest(player, cap);
        }
    }
    if snap.next_download_complete {
        let cap = player.with_state(|st| {
            st.prefetch
                .queued_mut()
                .and_then(|queued| queued.capturing.take())
        });
        if let Some(cap) = cap {
            crate::download::spawn_harvest(player, cap);
        }
    }
}

/// gapless 边界推进:曲终(`track_finished_seq` 前进)→ 收割旧曲 capture、完播打点,
/// 据是否真无缝(仍出声 + 有预排)采纳已预排曲([`adopt_queued`]),否则兜底 `play_song`(有间隙)。
pub(crate) fn check_advance(player: &PlayerCore) {
    let snap = player.audio_snapshot();
    if snap.track_finished_seq <= player.last_seen_finished_seq() {
        return;
    }
    player.set_last_seen_finished_seq(snap.track_finished_seq);

    // 曲终时 capture 还在 → 它没被 check_harvest(先于本函数跑,见 play.rs 调用序)按 download_complete
    // 收走 = 下载未真完成。半截 capture 无用(截断文件入缓存后回放会解码 IO 错),删残件、不入缓存。
    let old_cap = player.with_state(|st| st.capturing.take());
    if let Some(cap) = old_cap {
        drop(std::fs::remove_file(&cap.path));
    }

    let (old, has_queued) =
        player.with_state(|st| (st.current_song.clone(), st.prefetch.is_armed()));
    let boundary_song = old.as_ref().map(|s| s.id.clone());
    // 自然播完 = 听了整首;duration 未知时退用 position。
    if let Some(old) = old {
        let listen_ms = old.duration_ms.unwrap_or(snap.position_ms);
        player.spawn_on_played(old.id.clone(), mineral_stats::FinishReason::Eof, listen_ms);
        player
            .notify()
            .track_finished(&old, mineral_protocol::FinishReason::Eof);
    }

    let action = decide_advance(/*finished_advanced*/ true, snap.playing, has_queued);
    // adopt = 预排就位、引擎已无缝轮转;fallback = 预排没赶上,兜底 play_song(有间隙)。
    mineral_log::info!(
        target: "player",
        action = match action {
            Advance::Adopt => "adopt",
            Advance::Fallback => "fallback",
            Advance::None => "none",
        },
        playing = snap.playing,
        has_queued,
        finished_seq = snap.track_finished_seq,
        "gapless boundary"
    );
    // 埋点:无缝边界裁决(adopt=真无缝 / fallback=有间隙;None 不记)。
    if let Some(song) = boundary_song {
        let result = match action {
            Advance::Adopt => Some(mineral_stats::GaplessResult::Adopt),
            Advance::Fallback => Some(mineral_stats::GaplessResult::Fallback),
            Advance::None => None,
        };
        if let Some(result) = result {
            player.inner.stats.event(mineral_stats::StatsEvent::System(
                mineral_stats::SystemEvent::GaplessBoundary { song, result },
            ));
        }
    }
    match action {
        Advance::Adopt => {
            player.with_state(|st| {
                let _ = adopt_queued(st);
            });
            let (new, play_mode, playback_origin, media_info) = player.with_state(|st| {
                (
                    st.current_song.clone(),
                    st.play_mode,
                    st.play_origin,
                    st.media_info.clone(),
                )
            });
            if let Some(s) = new {
                // 埋点:无缝续播的新曲也是一次起播——adopt 不走 play_song,这里补起播快照,
                // 否则该曲只在下个边界拿到 play_ended(无匹配 pending)而彻底漏记。actor 单
                // 消费者按 FIFO 先消化前面 spawn_on_played 的 play_ended(old),再收本条。
                // context 经 take_play_context 与 play_song 同规矩:先消费 per-song 覆盖
                // (插队散曲经无缝接续也要记它自己的语境),否则继承队列级语境。
                let context = player.take_play_context(&s.id);
                if let Some(pending) = crate::pending_from_start(
                    s.clone(),
                    crate::stats_play_mode(play_mode),
                    s.duration_ms.and_then(|d| i64::try_from(d).ok()),
                    playback_origin.unwrap_or(PlaybackOrigin::Remote),
                    mineral_stats::PlayOrigin::AutoAdvance,
                    mineral_stats::Actor::System,
                    context,
                ) {
                    player.inner.stats.play_started(pending);
                }
                if let Some(info) = media_info {
                    player.enrich_from_media_info(&info);
                }
                player.submit_task(
                    TaskKind::ChannelFetch(ChannelFetchKind::Lyrics { song_id: s.id }),
                    Priority::User,
                );
            }
            // 无缝翻曲后补推新当前曲的 db 包络(预排时已算好;client 换曲后才认它)。
            player.replay_current_envelope();
            player.spawn_save_session();
        }
        Advance::Fallback => {
            // 清掉过期预排(+ 删其半截 capture 残件)+ 引擎里可能的待建 next,走兜底重播。
            let stale = player.with_state(State::take_prefetch);
            if let Some(cap) = stale.and_then(|q| q.capturing) {
                drop(std::fs::remove_file(&cap.path));
            }
            player.audio().clear_next();
            // 按下标推进 queue_sel(advance_next),play_song 据守卫保留它,重复曲不回退。
            let next = player.with_state(advance_next);
            if let Some(next) = next {
                player.play_song(
                    &next,
                    mineral_stats::PlayOrigin::AutoAdvance,
                    mineral_stats::Actor::System,
                );
            }
        }
        Advance::None => {}
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::{BitRate, PlaybackMediaInfo};
    use mineral_protocol::{PlayCursor, PlaybackOrigin};
    use mineral_test::song;

    use super::{Advance, Queued, adopt_queued, decide_advance, prefetch_window_open};
    use crate::playback_instance::PlaybackSlot;
    use crate::state::State;

    /// prefetch_window_open:decoder 实测优先;实测探不出(分片 fMP4 流式打开)回落元数据
    /// ——B站源曾因缺这层回落,预排窗口永远不开、gapless 从不触发;两口径都未知不开窗。
    #[test]
    fn prefetch_window_prefers_engine_falls_back_to_metadata() {
        assert!(
            prefetch_window_open(Some(200_000), None, 195_000, 10_000),
            "实测时长,窗口内应开"
        );
        assert!(
            !prefetch_window_open(Some(200_000), None, 100_000, 10_000),
            "实测时长,窗口外不开"
        );
        assert!(
            prefetch_window_open(None, Some(200_000), 195_000, 10_000),
            "实测缺失应回落元数据"
        );
        assert!(
            !prefetch_window_open(Some(300_000), Some(200_000), 195_000, 10_000),
            "两口径都有值时实测优先(顶换流元数据描述的是原源音频)"
        );
        assert!(
            !prefetch_window_open(None, None, 195_000, 10_000),
            "时长全未知不应开窗"
        );
    }

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
                capturing: None,
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
