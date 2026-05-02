//! 视口 prefetch:按 `sel ± [`RADIUS`]` 提前 fetch 用户即将看到的数据。
//!
//! 两件:
//! - **cover**:封面在右栏 focus 时显示
//! - **tracks**:歌单的 length 标签在 sidebar 列表上直接可见(`—` vs 真值)
//!
//! 两边都靠 scheduler 的 dedup 兜底重复请求,稳态下 tick 开销 = O(2·radius+1) hash 查找。

use mineral_model::MediaUrl;
use mineral_task::{ChannelFetchKind, Priority, Scheduler, TaskKind};

use crate::state::{AppState, View};

/// 各 prefetch 默认半径。覆盖典型 viewport(~30 行)+ 几次 `Shift+J/K` 跳跃
/// (每次 7 行)的 lookahead。两件 prefetch(cover / playlist tracks)统一用同一值,
/// 后续接 config 时再分开调。
const RADIUS: usize = 64;

/// 每 tick 调一次:封面 + 歌单 tracks 两路 prefetch。
pub fn tick(state: &mut AppState, scheduler: &Scheduler) {
    request_covers(state, scheduler);
    request_playlist_tracks(state, scheduler);
}

/// 看 view 决定的 sel 周围 [`RADIUS`] 内未 cache / pending 的封面 URL,
/// sel 优先 → 外扩 提交。
fn request_covers(state: &mut AppState, scheduler: &Scheduler) {
    let urls = collect_pending_covers(state);
    for url in urls {
        ensure_cover(state, scheduler, url);
    }
}

fn collect_pending_covers(state: &AppState) -> Vec<MediaUrl> {
    let mut out = Vec::<MediaUrl>::new();
    let cache = &state.cover_cache;
    let pending = &state.cover_pending;
    let push_if_new = |opt: Option<&MediaUrl>, out: &mut Vec<MediaUrl>| {
        if let Some(u) = opt
            && !cache.contains_key(u)
            && !pending.contains(u)
            && !out.contains(u)
        {
            out.push(u.clone());
        }
    };
    match state.view {
        View::Playlists => {
            // sel 是 filtered 索引,prefetch 邻居一律走 filtered,免得跟可视窗口错位。
            let filtered = state.filtered_playlists();
            let sel = state.sel_playlist;
            let get = |i: usize| -> Option<&MediaUrl> {
                filtered.get(i).and_then(|p| p.data.cover_url.as_ref())
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
            push_if_new(
                filtered.get(sel).and_then(|sv| sv.data.cover_url.as_ref()),
                &mut out,
            );
            for d in 1..=RADIUS {
                if let Some(idx) = sel.checked_sub(d) {
                    push_if_new(
                        filtered.get(idx).and_then(|sv| sv.data.cover_url.as_ref()),
                        &mut out,
                    );
                }
                push_if_new(
                    filtered
                        .get(sel.saturating_add(d))
                        .and_then(|sv| sv.data.cover_url.as_ref()),
                    &mut out,
                );
            }
        }
    }
    out
}

fn ensure_cover(state: &mut AppState, scheduler: &Scheduler, url: MediaUrl) {
    if state.cover_cache.contains_key(&url) || state.cover_pending.contains(&url) {
        return;
    }
    state.cover_pending.insert(url.clone());
    scheduler.submit(TaskKind::CoverArt { url }, Priority::User);
}

/// 看 sel_playlist 周围 [`RADIUS`] 内未 cache 的歌单,提交 PlaylistTracks。
/// 只在 Playlists view 下生效 —— Library view 的当前 playlist 一定已经 cache(进 view 的前提)。
fn request_playlist_tracks(state: &AppState, scheduler: &Scheduler) {
    if state.view != View::Playlists {
        return;
    }
    let filtered = state.filtered_playlists();
    let sel = state.sel_playlist;
    let submit = |idx: usize| {
        let Some(p) = filtered.get(idx) else {
            return;
        };
        if state.tracks_cache.contains_key(&p.data.id) {
            return;
        }
        scheduler.submit(
            TaskKind::ChannelFetch(ChannelFetchKind::PlaylistTracks {
                source: p.data.source,
                id: p.data.id.clone(),
            }),
            Priority::User,
        );
    };
    submit(sel);
    for d in 1..=RADIUS {
        if let Some(idx) = sel.checked_sub(d) {
            submit(idx);
        }
        submit(sel.saturating_add(d));
    }
}
