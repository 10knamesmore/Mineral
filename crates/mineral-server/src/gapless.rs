//! 服务端 gapless 编排:把「已预排的下一曲」状态在无缝边界处扶正为「当前曲」,
//! 以及预排 / 并发 capture 收割相关的纯状态变换。
//!
//! 引擎([`mineral_audio`])在当前曲自然耗尽时已把下一曲零静音接上;服务端这边只需在
//! 边界处把记账状态轮转过来(current=queued、queue_sel 推进、play_url/origin/capturing
//! 轮转、歌词与预拉复位),**不**重新 `play_song`(音频没有中断)。

use std::path::PathBuf;

use mineral_model::{BitRate, MediaUrl, PlayUrl, Song, SongId};
use mineral_protocol::PlaybackOrigin;
use mineral_task::{ChannelFetchKind, Priority, TaskKind};

use crate::download::Capturing;
use crate::player::PlayerCore;
use crate::queue::{advance_next, next_in_queue, next_index};
use crate::state::State;

/// 一首「已预排进 rodio 队列、等当前曲播完接续」的下一曲及其播放记账。
pub(crate) struct Queued {
    /// 预排的下一曲。
    pub(crate) song: Song,

    /// 该曲播放 URL(本地命中或远端取链;`None` 表示尚未填)。
    pub(crate) play_url: Option<PlayUrl>,

    /// 该曲来源(下载 / 缓存 / 远端),边界轮转时顶进 `play_origin`。
    pub(crate) origin: PlaybackOrigin,

    /// 该曲的 capture 上下文(远端可缓存时 `Some`;RepeatOne 同曲循环 / 本地命中为 `None`)。
    pub(crate) capturing: Option<Capturing>,
}

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
/// current=queued、queue_sel 推进到它在队列的位置、play_url/origin/capturing 轮转、
/// 歌词与预拉状态复位。
///
/// # Params:
///   - `st`: 播放状态(原地轮转)
///
/// # Return:
///   被顶替的旧当前歌 id(供打点);无已预排曲则 `None` 且不改状态。
pub(crate) fn adopt_queued(st: &mut State) -> Option<SongId> {
    let queued = st.queued.take()?;
    let old_id = st.current_song.as_ref().map(|s| s.id.clone());
    // queue_sel 此刻仍指旧当前曲;预排曲就是当时 next_index 算出的那一首(队列一变即作废预排),
    // 故按下标推进,**不**按 queued.song 身份 first-match——重复曲会把下标吸附到首个副本。
    if let Some(idx) = next_index(st) {
        st.queue_sel = idx;
    }
    st.current_song = Some(queued.song);
    st.play_url = queued.play_url;
    st.play_origin = Some(queued.origin);
    st.capturing = queued.capturing;
    st.current_lyrics = None;
    st.current_lyrics_song_id = None;
    st.prefetch_fired_for = None;
    st.bump_current();
    old_id
}

/// gapless 预排:进入曲终前窗口(配置 `daemon.gapless_prefetch_ms`)时,据下一曲来源预排 decoder 进引擎队列
/// ——本地命中 / RepeatOne 直排,远端先取链 → [`on_prefetch_url_ready`] 再排。本曲只触发一次。
pub(crate) fn check_prefetch(player: &PlayerCore) {
    let snap = player.audio_snapshot();
    if snap.duration_ms == 0 {
        return;
    }
    if snap.duration_ms.saturating_sub(snap.position_ms) > player.gapless_prefetch_ms() {
        return;
    }
    let (cur_id, next) = player.with_state(|st| {
        let Some(cur_id) = st.current_song.as_ref().map(|s| s.id.clone()) else {
            return (None, None);
        };
        // 已排好下一曲 → 不重复。
        if st.queued.is_some() {
            return (Some(cur_id), None);
        }
        let next = next_in_queue(st);
        // 已对这首 next 发起过预拉 → 不重复(prefetch_fired_for 记的是正在预拉的下一曲 id)。
        if let Some(n) = next.as_ref()
            && st.prefetch_fired_for.as_ref() == Some(&n.id)
        {
            return (Some(cur_id), None);
        }
        (Some(cur_id), next)
    });
    let (Some(cur_id), Some(next)) = (cur_id, next) else {
        return;
    };
    player.with_state(|st| st.prefetch_fired_for = Some(next.id.clone()));

    if next.id == cur_id {
        // RepeatOne:同曲循环。
        queue_repeatone(player, next);
    } else if let Some((path, quality, origin)) = crate::resolve::resolve_local(
        player.media_cache(),
        player.music_dir(),
        &next,
        player.playback_quality(),
    ) {
        queue_local_next(player, next, path, quality, origin);
    } else {
        player.submit_task(
            TaskKind::ChannelFetch(ChannelFetchKind::SongUrl {
                song_id: next.id,
                quality: player.playback_quality(),
            }),
            Priority::Background,
        );
    }
}

/// RepeatOne 循环预排:复用当前曲 play_url 直接预排,**capture 传 None**——首遍播放已在缓存,
/// 复用同一临时路径的第二路写会撞坏在写文件(取舍 3)。
fn queue_repeatone(player: &PlayerCore, next: Song) {
    let (pu, origin) = player.with_state(|st| (st.play_url.clone(), st.play_origin));
    let Some(pu) = pu else {
        return; // 当前 url 尚未就绪(极少),本轮不排。
    };
    player
        .audio()
        .append_next(pu.url.clone(), pu.stream_headers.clone());
    player.with_state(|st| {
        st.queued = Some(Queued {
            song: next,
            play_url: Some(pu),
            origin: origin.unwrap_or(PlaybackOrigin::Remote),
            capturing: None,
        });
    });
}

/// 本地命中的下一曲:直接以本地路径预排(已在缓存 / 下载库,无需 capture)。
fn queue_local_next(
    player: &PlayerCore,
    next: Song,
    path: PathBuf,
    quality: BitRate,
    origin: PlaybackOrigin,
) {
    let pu = crate::resolve::local_play_url(&next, &path, quality);
    // 本地文件无需附加取流头。
    player
        .audio()
        .append_next(MediaUrl::Local(path), Vec::new());
    player.with_state(|st| {
        st.queued = Some(Queued {
            song: next,
            play_url: Some(pu),
            origin,
            capturing: None,
        });
    });
}

/// 远端预排曲取链就绪:据缓存可用与否带 / 不带 capture 预排,登记 [`Queued`]。
/// 队列已变(找不到该曲)则丢弃。
///
/// # Params:
///   - `song_id`: 取链回来的曲 id(应为已发起预排的下一曲)
///   - `play_url`: 取到的播放 URL
pub(crate) fn on_prefetch_url_ready(player: &PlayerCore, song_id: &SongId, play_url: PlayUrl) {
    let next = player.with_state(|st| st.queue.iter().find(|s| s.id == *song_id).cloned());
    let Some(next) = next else {
        return;
    };
    match player
        .media_cache()
        .capture_path(&next.id, player.playback_quality())
    {
        Some(path) => {
            player.audio().append_next_capturing(
                play_url.url.clone(),
                play_url.stream_headers.clone(),
                path.clone(),
            );
            let cap = Capturing {
                song: next.clone(),
                quality: player.playback_quality(),
                format: play_url.format.clone(),
                path,
            };
            player.with_state(|st| {
                st.queued = Some(Queued {
                    song: next,
                    play_url: Some(play_url),
                    origin: PlaybackOrigin::Remote,
                    capturing: Some(cap),
                });
            });
        }
        None => {
            player
                .audio()
                .append_next(play_url.url.clone(), play_url.stream_headers.clone());
            player.with_state(|st| {
                st.queued = Some(Queued {
                    song: next,
                    play_url: Some(play_url),
                    origin: PlaybackOrigin::Remote,
                    capturing: None,
                });
            });
        }
    }
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
        let cap = player.with_state(|st| st.queued.as_mut().and_then(|q| q.capturing.take()));
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

    // 旧当前曲自然播完 → capture 必已下完 → 收割(check_harvest 没赶上时兜底)。
    let old_cap = player.with_state(|st| st.capturing.take());
    if let Some(cap) = old_cap {
        crate::download::spawn_harvest(player, cap);
    }

    let (old, has_queued) = player.with_state(|st| (st.current_song.clone(), st.queued.is_some()));
    // 自然播完 = 听了整首;duration 未知时退用 position。
    if let Some(old) = old {
        let listen_ms = if old.duration_ms == 0 {
            snap.position_ms
        } else {
            old.duration_ms
        };
        player.spawn_on_played(old.id.clone(), /*completed*/ true, listen_ms);
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
    match action {
        Advance::Adopt => {
            player.with_state(|st| {
                let _ = adopt_queued(st);
            });
            let new = player.with_state(|st| st.current_song.clone());
            if let Some(s) = new {
                player.submit_task(
                    TaskKind::ChannelFetch(ChannelFetchKind::Lyrics { song_id: s.id }),
                    Priority::User,
                );
            }
            player.spawn_save_session();
        }
        Advance::Fallback => {
            // 清掉过期预排(+ 删其半截 capture 残件)+ 引擎里可能的待建 next,走兜底重播。
            let stale = player.with_state(|st| st.queued.take());
            if let Some(cap) = stale.and_then(|q| q.capturing) {
                drop(std::fs::remove_file(&cap.path));
            }
            player.audio().clear_next();
            // 按下标推进 queue_sel(advance_next),play_song 据守卫保留它,重复曲不回退。
            let next = player.with_state(advance_next);
            if let Some(next) = next {
                player.play_song(&next);
            }
        }
        Advance::None => {}
    }
}

#[cfg(test)]
mod tests {
    use mineral_protocol::PlaybackOrigin;
    use mineral_test::song;

    use super::{Advance, Queued, adopt_queued, decide_advance};
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
        st.queue_sel = 0;
        st.current_song = Some(song("a"));
        st.prefetch_fired_for = Some(song("a").id);
        st.queued = Some(Queued {
            song: song("b"),
            play_url: None,
            origin: PlaybackOrigin::Remote,
            capturing: None,
        });

        let old = adopt_queued(&mut st);
        assert_eq!(old, Some(song("a").id), "应返回被顶替的旧当前歌 id");
        assert_eq!(
            st.current_song.as_ref().map(|s| s.id.clone()),
            Some(song("b").id),
            "current 应变成 queued"
        );
        assert_eq!(st.queue_sel, 1, "queue_sel 应定位到 b");
        assert_eq!(st.play_origin, Some(PlaybackOrigin::Remote));
        assert!(st.queued.is_none(), "queued 应被取走");
        assert!(st.prefetch_fired_for.is_none(), "预拉触发标记应复位");
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
