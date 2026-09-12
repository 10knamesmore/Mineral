//! 预排窗口、下一曲媒体装填与预取裁决记录。

use mineral_model::{Song, SongId};
use mineral_playback::{DirectMedia, OpenedMedia};
use mineral_protocol::PlaybackOrigin;

use super::Queued;
use crate::playback_instance::PlaybackSlot;
use crate::player::PlayerCore;
use crate::queue::next_in_queue;

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

/// 预取来源按落库口径归并:本地副本(缓存 / 下载库)→ `Local`,远端流 → `Remote`。
///
/// # Params:
///   - `origin`: 播放来源事实
///
/// # Return:
///   对应的预取来源。
pub(crate) fn prefetch_source(origin: PlaybackOrigin) -> mineral_stats::PrefetchSource {
    match origin {
        PlaybackOrigin::Cache | PlaybackOrigin::Download => mineral_stats::PrefetchSource::Local,
        PlaybackOrigin::Remote => mineral_stats::PrefetchSource::Remote,
    }
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
pub(crate) fn arm_opened(
    player: &PlayerCore,
    song: Song,
    slot: &PlaybackSlot,
    opened: OpenedMedia,
    direct: Option<DirectMedia>,
    origin: PlaybackOrigin,
) {
    let info = opened.info().clone();
    let armed = player.with_state(|st| {
        let Some(active) = st.prefetch.take_opening(slot.instance_id, &song.id) else {
            return false;
        };
        player.audio().append_next(opened);
        st.prefetch.arm(
            active,
            Queued {
                song: song.clone(),
                media_info: info.clone(),
                direct_media: direct,
                origin,
            },
        );
        true
    });
    if !armed {
        return;
    }
    let source = prefetch_source(origin);
    record_prefetch(
        player,
        song.id,
        source,
        mineral_stats::PrefetchResolution::Armed,
    );
}

#[cfg(test)]
mod tests {
    use super::prefetch_window_open;

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
}
