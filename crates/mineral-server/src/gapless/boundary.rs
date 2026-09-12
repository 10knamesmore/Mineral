//! 无缝边界的播放记账、事件与歌词及包络续接。
//!
//! 引擎在当前曲自然耗尽时已把下一曲零静音接上；边界处只轮转记账状态
//! (current=queued、queue_sel 推进、resolved/origin 轮转、歌词与预拉复位)，
//! 不重新 `play_song`，音频没有中断。

use mineral_protocol::PlaybackOrigin;
use mineral_task::{ChannelFetchKind, Priority, TaskKind};

use super::{Advance, adopt_queued, decide_advance};
use crate::player::PlayerCore;
use crate::queue::advance_next;
use crate::state::State;

/// gapless 边界推进:曲终(`track_finished_seq` 前进)→ 完播打点,
/// 据是否真无缝(仍出声 + 有预排)采纳已预排曲([`adopt_queued`]),否则兜底 `play_song`(有间隙)。
pub(crate) fn check_advance(player: &PlayerCore) {
    let snap = player.audio_snapshot();
    if snap.track_finished_seq <= player.last_seen_finished_seq() {
        return;
    }
    player.set_last_seen_finished_seq(snap.track_finished_seq);

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
            // 清掉过期预排和引擎里可能的待建 next，走兜底重播。
            drop(player.with_state(State::take_prefetch));
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
