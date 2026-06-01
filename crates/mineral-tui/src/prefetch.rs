//! 视口 prefetch:按 `sel ± [`RADIUS`]` 提前 fetch 用户即将看到的数据。
//!
//! 两件:
//! - **cover**:封面在右栏 focus 时显示
//! - **tracks**:歌单的 length 标签在 sidebar 列表上直接可见(`—` vs 真值)
//!
//! 两边都靠 scheduler 的 dedup 兜底重复请求,稳态下 tick 开销 = O(2·radius+1) hash 查找。

use std::time::Duration;

use mineral_model::{MediaUrl, PlaylistId, SongId, SourceKind};
use mineral_server::Client;
use mineral_task::{ChannelFetchKind, Priority, TaskKind};

use crate::cover::CoverFetcher;
use crate::state::{AppState, View};

/// 各 prefetch 默认半径。覆盖典型 viewport(~30 行)+ 几次 `Shift+J/K` 跳跃
/// (每次 7 行)的 lookahead。两件 prefetch(cover / playlist tracks)统一用同一值,
/// 后续接 config 时再分开调。
const RADIUS: usize = 64;

/// 选中某首歌停留超过此窗口,才查它的远端真实播放次数。比 [`crate::state::COVER_DEBOUNCE`]
/// 长得多 —— 回忆坐标单首一请求且可能撞风控,只在用户「停下来看」时才打。后续按手感调。
const PLAY_COUNT_DEBOUNCE: Duration = Duration::from_millis(500);

/// 每 tick 调一次:封面 + 歌单 tracks + 选中歌远端播放次数三路 prefetch。
pub fn tick(state: &mut AppState, client: &dyn Client, covers: &CoverFetcher) {
    request_covers(state, covers);
    request_playlist_tracks(state, client);
    request_play_count(state, client);
}

/// 看 view 决定的 sel 周围 [`RADIUS`] 内未 cache / pending 的封面,
/// sel 优先 → 外扩 提交给 client 端 fetcher。来源随封面一起带出(决定落盘子目录)。
fn request_covers(state: &mut AppState, covers: &CoverFetcher) {
    let items = collect_pending_covers(state);
    for (source, url) in items {
        ensure_cover(state, covers, source, url);
    }
}

/// 收集当前 view 下「sel + 邻居 (±RADIUS)」中未 cache、未 pending 的 `(来源, 封面 URL)`。
///
/// 来源从所在条目的 id namespace 派生(歌单 / 歌曲都带源)。
fn collect_pending_covers(state: &AppState) -> Vec<(SourceKind, MediaUrl)> {
    let mut out = Vec::<(SourceKind, MediaUrl)>::new();
    let cache = &state.cover_cache;
    let pending = &state.cover_pending;
    let push_if_new = |item: Option<(SourceKind, &MediaUrl)>,
                       out: &mut Vec<(SourceKind, MediaUrl)>| {
        if let Some((source, u)) = item
            && !cache.contains_key(u)
            && !pending.contains(u)
            && !out.iter().any(|(_, existing)| existing == u)
        {
            out.push((source, u.clone()));
        }
    };
    match state.view {
        View::Playlists => {
            // sel 是 filtered 索引,prefetch 邻居一律走 filtered,免得跟可视窗口错位。
            let filtered = state.filtered_playlists();
            let sel = state.sel_playlist;
            let get = |i: usize| -> Option<(SourceKind, &MediaUrl)> {
                filtered.get(i).and_then(|p| {
                    p.data
                        .cover_url
                        .as_ref()
                        .map(|u| (p.data.id.namespace(), u))
                })
            };
            push_if_new(get(sel), &mut out);
            for d in 1..=RADIUS {
                if let Some(idx) = sel.checked_sub(d) {
                    push_if_new(get(idx), &mut out);
                }
                push_if_new(get(sel.saturating_add(d)), &mut out);
            }
        }
        View::Library => {
            // sel 是 filtered 索引,sel-first + 邻居全走 filtered_tracks(SongView Vec
            // clone <200 行 typical, <1ms),保持索引语义一致。
            let filtered = state.filtered_tracks();
            let sel = state.sel_track;
            let get = |i: usize| -> Option<(SourceKind, &MediaUrl)> {
                filtered.get(i).and_then(|sv| {
                    sv.data
                        .cover_url
                        .as_ref()
                        .map(|u| (sv.data.id.namespace(), u))
                })
            };
            push_if_new(get(sel), &mut out);
            for d in 1..=RADIUS {
                if let Some(idx) = sel.checked_sub(d) {
                    push_if_new(get(idx), &mut out);
                }
                push_if_new(get(sel.saturating_add(d)), &mut out);
            }
        }
    }
    out
}

/// 把 `url` 标 pending 并丢给 [`CoverFetcher`];已 cache 或已 pending 时直接返回。
/// `source` 随请求带给 fetcher(决定缓存落盘子目录)。
fn ensure_cover(state: &mut AppState, covers: &CoverFetcher, source: SourceKind, url: MediaUrl) {
    if state.cover_cache.contains_key(&url) || state.cover_pending.contains(&url) {
        return;
    }
    state.cover_pending.insert(url.clone());
    mineral_log::debug!(target: "prefetch", url = %url, source = ?source, "request cover");
    covers.request(source, url);
}

/// 看 sel_playlist 周围 [`RADIUS`] 内未 cache 的歌单,提交 PlaylistTracks。
/// 只在 Playlists view 下生效 —— Library view 的当前 playlist 一定已经 cache(进 view 的前提)。
fn request_playlist_tracks(state: &mut AppState, client: &dyn Client) {
    if state.view != View::Playlists {
        return;
    }
    for id in collect_pending_tracks(state) {
        mineral_log::debug!(target: "prefetch", playlist_id = id.as_str(), source = ?id.namespace(), "request playlist tracks");
        client.submit_task(
            TaskKind::ChannelFetch(ChannelFetchKind::PlaylistTracks { id: id.clone() }),
            Priority::User,
        );
        // 成败都记:失败歌单的 tracks_cache 永远不会被填,只有靠这里去重才不会
        // 每帧重提交(scheduler dedup 只在任务进行中有效,失败瞬间完成就失效)。
        state.tracks_requested.insert(id);
    }
}

/// 选中某首歌停留超过 [`PLAY_COUNT_DEBOUNCE`] 后,查它的远端真实累计播放次数。
///
/// 只查当前选中那一首(回忆坐标单首一请求,且只在 selected 详情展示);停留防抖
/// 避免翻列表时为掠过的歌打满 API。`play_count_requested` 成败都记,不反复打同一首。
/// 不预判来源能力 —— 不支持的源任务在 lane 里静默失败(debug),不把 channel 能力硬编码进 UI。
fn request_play_count(state: &mut AppState, client: &dyn Client) {
    if state.view != View::Library {
        return;
    }
    if state.last_sel_change.elapsed() < PLAY_COUNT_DEBOUNCE {
        return;
    }
    let Some(id) = selected_track_id(state) else {
        return;
    };
    if state.play_count_requested.contains(&id) {
        return;
    }
    mineral_log::debug!(target: "prefetch", song_id = id.as_str(), source = ?id.namespace(), "request remote play count");
    client.submit_task(
        TaskKind::ChannelFetch(ChannelFetchKind::RemotePlayCount {
            song_id: id.clone(),
        }),
        Priority::User,
    );
    state.play_count_requested.insert(id);
}

/// 当前 Library 选中行的歌曲 id(filtered 索引);无选中 / 空列表返回 `None`。
fn selected_track_id(state: &AppState) -> Option<SongId> {
    state
        .filtered_tracks()
        .get(state.sel_track)
        .map(|sv| sv.data.id.clone())
}

/// sel 周围 [`RADIUS`] 内、既未 cache 也未请求过的歌单(sel 优先,再向两侧外扩)。
fn collect_pending_tracks(state: &AppState) -> Vec<PlaylistId> {
    let filtered = state.filtered_playlists();
    let sel = state.sel_playlist;
    let mut out = Vec::new();
    let mut consider = |idx: usize| {
        if let Some(p) = filtered.get(idx) {
            let id = &p.data.id;
            if !state.tracks_cache.contains_key(id) && !state.tracks_requested.contains(id) {
                out.push(id.clone());
            }
        }
    };
    consider(sel);
    for d in 1..=RADIUS {
        if let Some(idx) = sel.checked_sub(d) {
            consider(idx);
        }
        consider(sel.saturating_add(d));
    }
    out
}
