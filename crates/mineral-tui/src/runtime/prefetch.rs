//! 视口 prefetch:按 `sel ± [`RADIUS`]` 提前 fetch 用户即将看到的数据。
//!
//! 两件:
//! - **cover**:封面在右栏 focus 时显示
//! - **tracks**:歌单的 length 标签在 sidebar 列表上直接可见(`—` vs 真值)
//!
//! 两边都靠 scheduler 的 dedup 兜底重复请求,稳态下 tick 开销 = O(2·radius+1) hash 查找。

use mineral_channel_core::Page;
use mineral_model::{MediaUrl, PlaylistId, Song, SongId, SourceKind};
use mineral_server::Client;
use mineral_task::{ChannelFetchKind, Priority, TaskKind};

use crate::runtime::cover_fetch::CoverFetcher;
use crate::runtime::state::{AppState, DetailFetch, View};

/// 每 tick 调一次:封面 + 歌单 tracks + 选中歌远端播放次数三路 prefetch。
pub fn tick(state: &mut AppState, client: &dyn Client, covers: &CoverFetcher) {
    request_covers(state, covers);
    request_playlist_tracks(state, client);
    request_play_count(state, client);
    request_detail(state, client, covers);
    request_detail_selected_cover(state, covers);
}

/// 看 view 决定的 sel 周围 `prefetch.radius` 内未 cache / pending 的封面,
/// sel 优先 → 外扩 提交给 client 端 fetcher。来源随封面一起带出(决定落盘子目录)。
fn request_covers(state: &mut AppState, covers: &CoverFetcher) {
    let items = collect_pending_covers(state);
    for (source, url) in items {
        ensure_cover(state, covers, source, url);
    }
}

/// 收集未 cache、未 pending 的 `(来源, 封面 URL)`,两条轴:浏览选中 sel ± `prefetch.radius`
/// (随 view 取歌单 / 歌曲列表),以及在播曲 ± `prefetch.playback_cover_radius`(沿播放队列)。
/// 后者与 view 无关——全屏渲染的是在播曲,且自动切歌的下一首也要先就绪。
///
/// 来源从所在条目的 id namespace 派生(歌单 / 歌曲都带源)。
fn collect_pending_covers(state: &AppState) -> Vec<(SourceKind, MediaUrl)> {
    let radius = *state.cfg.tui().prefetch().radius();
    let playback_radius = *state.cfg.tui().prefetch().playback_cover_radius();
    let mut out = Vec::<(SourceKind, MediaUrl)>::new();
    let cache = &state.covers.cache;
    let pending = &state.covers.pending;
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
    match state.view.current() {
        View::Playlists => {
            // sel 是 filtered 索引,prefetch 邻居一律走 filtered,免得跟可视窗口错位。
            let filtered = state.filtered_playlists();
            let sel = state.nav.sel_playlist;
            let get = |i: usize| -> Option<(SourceKind, &MediaUrl)> {
                filtered.get(i).and_then(|p| {
                    p.data
                        .cover_url
                        .as_ref()
                        .map(|u| (p.data.id.namespace(), u))
                })
            };
            push_if_new(get(sel), &mut out);
            for d in 1..=radius {
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
            let sel = state.nav.sel_track;
            let get = |i: usize| -> Option<(SourceKind, &MediaUrl)> {
                filtered.get(i).and_then(|sv| {
                    sv.data
                        .cover_url
                        .as_ref()
                        .map(|u| (sv.data.id.namespace(), u))
                })
            };
            push_if_new(get(sel), &mut out);
            for d in 1..=radius {
                if let Some(idx) = sel.checked_sub(d) {
                    push_if_new(get(idx), &mut out);
                }
                push_if_new(get(sel.saturating_add(d)), &mut out);
            }
        }
    }

    // 在播曲与浏览选中解耦:全屏直接渲染在播曲,自动切歌也要让接下来几首封面就绪。沿
    // `state.player.queue`(已应用 shuffle 的有效播放顺序)给在播曲 ± `playback_cover_radius`
    // 预取;在播曲自身即便不在队列(单首试听 / 队列刚换)也单独保一张。
    if let Some(track) = state.playback.track.as_ref() {
        push_if_new(song_cover(track), &mut out);
    }
    if let Some(pos) = state.queue_current_index() {
        for d in 1..=playback_radius {
            if let Some(idx) = pos.checked_sub(d)
                && let Some(s) = state.player.queue.get(idx)
            {
                push_if_new(song_cover(s), &mut out);
            }
            if let Some(s) = state.player.queue.get(pos.saturating_add(d)) {
                push_if_new(song_cover(s), &mut out);
            }
        }
    }
    out
}

/// 从一首歌取 `(来源, 封面 URL)`;无封面返回 `None`。来源由 id namespace 派生。
fn song_cover(s: &Song) -> Option<(SourceKind, &MediaUrl)> {
    s.cover_url.as_ref().map(|u| (s.id.namespace(), u))
}

/// 把 `url` 标 pending 并丢给 [`CoverFetcher`];已 cache 或已 pending 时直接返回。
/// `source` 随请求带给 fetcher(决定缓存落盘子目录)。
fn ensure_cover(state: &mut AppState, covers: &CoverFetcher, source: SourceKind, url: MediaUrl) {
    if state.covers.cache.contains_key(&url) || state.covers.pending.contains(&url) {
        return;
    }
    state.covers.pending.insert(url.clone());
    mineral_log::debug!(target: "prefetch", url = %url, source = ?source, "request cover");
    covers.request(source, url);
}

/// 看 sel_playlist 周围 `prefetch.radius` 内未 cache 的歌单,提交 PlaylistDetail。
/// 只在 Playlists view 下生效 —— Library view 的当前 playlist 一定已经 cache(进 view 的前提)。
fn request_playlist_tracks(state: &mut AppState, client: &dyn Client) {
    if state.view != View::Playlists {
        return;
    }
    for id in collect_pending_tracks(state) {
        mineral_log::debug!(target: "prefetch", playlist_id = id.as_str(), source = ?id.namespace(), "request playlist tracks");
        client.submit_task(
            TaskKind::ChannelFetch(ChannelFetchKind::PlaylistDetail { id: id.clone() }),
            Priority::User,
        );
        // 成败都记:失败歌单的 library.tracks 永远不会被填,只有靠这里去重才不会
        // 每帧重提交(scheduler dedup 只在任务进行中有效,失败瞬间完成就失效)。
        state.library.tracks_requested.insert(id);
    }
}

/// 选中某首歌停留超过 `prefetch.play_count_debounce_ms` 后,查它的远端真实累计播放次数。
///
/// 只查当前选中那一首(回忆坐标单首一请求,且只在 selected 详情展示);停留防抖
/// 避免翻列表时为掠过的歌打满 API。`play_count_requested` 成败都记,不反复打同一首。
/// 不预判来源能力 —— 不支持的源任务在 lane 里静默失败(debug),不把 channel 能力硬编码进 UI。
fn request_play_count(state: &mut AppState, client: &dyn Client) {
    if state.view != View::Library {
        return;
    }
    let debounce =
        std::time::Duration::from_millis(*state.cfg.tui().prefetch().play_count_debounce_ms());
    if state.nav.last_sel_change.elapsed() < debounce {
        return;
    }
    let Some(id) = selected_track_id(state) else {
        return;
    };
    if state.library.play_count_requested.contains(&id) {
        return;
    }
    mineral_log::debug!(target: "prefetch", song_id = id.as_str(), source = ?id.namespace(), "request remote play count");
    client.submit_task(
        TaskKind::ChannelFetch(ChannelFetchKind::RemotePlayCount {
            song_id: id.clone(),
        }),
        Priority::User,
    );
    state.library.play_count_requested.insert(id);
}

/// search 布局态下，结果列/详情光标停留超防抖窗后，给当前 detail 栈顶帧补拉列表/详情
/// （Background 优先级），并把该实体封面搭车投给 fetcher。
///
/// 同帧只派一次（`DetailFrame.requested`）；移光标 / 下钻换新帧后可再派——失败的帧换走
/// 再回即重试（驻留窗口重新触发），与 spec「预览失败驻留重试」一致。布局态未开则不派。
fn request_detail(state: &mut AppState, client: &dyn Client, covers: &CoverFetcher) {
    if !state.channel_search.active.on() {
        return;
    }
    let debounce =
        std::time::Duration::from_millis(*state.cfg.tui().prefetch().play_count_debounce_ms());
    if state.nav.last_sel_change.elapsed() < debounce {
        return;
    }
    // 取出当前帧的拉取意图 + 封面并标记已派，随即释放 channel_search 借用。
    let intent = {
        let Some(kr) = state.channel_search.active_results_mut() else {
            return;
        };
        let Some(frame) = kr.detail.current_mut() else {
            return;
        };
        if !frame.needs_fetch() {
            return;
        }
        frame.mark_requested();
        frame
            .entity
            .fetch()
            .map(|fetch| (fetch, frame.entity.cover().cloned()))
    };
    // 单曲无所属专辑：已标记、跳过（降级只画歌曲卡片）。
    let Some((fetch, cover)) = intent else {
        return;
    };
    let source = fetch.source();
    mineral_log::debug!(target: "prefetch", ?source, key = %fetch.dedup_key(), "request detail");
    submit_detail_tasks(client, fetch);
    if let Some(url) = cover {
        ensure_cover(state, covers, source, url);
    }
}

/// search 布局态下，给当前 detail 帧列表选中项的封面搭车投 fetcher（artist 帧右栏副头图用）。
///
/// 与 detail fetch 去重解耦：选中项随 `[ ]` 切区 / 光标移动而变，每 tick 看一眼，靠
/// [`ensure_cover`] 的 cache/pending 去重兜重复。沿用 detail 驻留防抖窗，避免快速翻列表时
/// 给 fetcher 灌一堆滚过即弃的图；不投则右栏副头图永远停在程序化占位。
fn request_detail_selected_cover(state: &mut AppState, covers: &CoverFetcher) {
    if !state.channel_search.active.on() {
        return;
    }
    let debounce =
        std::time::Duration::from_millis(*state.cfg.tui().prefetch().play_count_debounce_ms());
    if state.nav.last_sel_change.elapsed() < debounce {
        return;
    }
    // 先取出 (source, url) 再释放 channel_search 借用，避免与 ensure_cover 的 &mut 冲突。
    // 来源用所在 artist 帧的 fetch source（选中歌/专辑与 artist 同 channel，落盘子目录一致）。
    let intent = {
        let Some(kr) = state.channel_search.active_results() else {
            return;
        };
        let Some(frame) = kr.detail.current() else {
            return;
        };
        match (frame.selected_cover().cloned(), frame.entity.fetch()) {
            (Some(url), Some(fetch)) => Some((fetch.source(), url)),
            _ => None,
        }
    };
    if let Some((source, url)) = intent {
        ensure_cover(state, covers, source, url);
    }
}

/// 按 [`DetailFetch`] 派对应的 channel 拉取任务（歌手两路：详情 + 专辑列表；其余单路）。
fn submit_detail_tasks(client: &dyn Client, fetch: DetailFetch) {
    match fetch {
        DetailFetch::AlbumDetail(id) => {
            client.submit_task(
                TaskKind::ChannelFetch(ChannelFetchKind::AlbumDetail { id }),
                Priority::Background,
            );
        }
        DetailFetch::PlaylistDetail(id) => {
            client.submit_task(
                TaskKind::ChannelFetch(ChannelFetchKind::PlaylistDetail { id }),
                Priority::Background,
            );
        }
        DetailFetch::Artist(id) => {
            client.submit_task(
                TaskKind::ChannelFetch(ChannelFetchKind::ArtistDetail { id: id.clone() }),
                Priority::Background,
            );
            client.submit_task(
                TaskKind::ChannelFetch(ChannelFetchKind::ArtistAlbums {
                    id,
                    page: Page::default(),
                }),
                Priority::Background,
            );
        }
    }
}

/// 当前 Library 选中行的歌曲 id(filtered 索引);无选中 / 空列表返回 `None`。
fn selected_track_id(state: &AppState) -> Option<SongId> {
    state
        .filtered_tracks()
        .get(state.nav.sel_track)
        .map(|sv| sv.data.id.clone())
}

/// sel 周围 `prefetch.radius` 内、既未 cache 也未请求过的歌单(sel 优先,再向两侧外扩)。
fn collect_pending_tracks(state: &AppState) -> Vec<PlaylistId> {
    let radius = *state.cfg.tui().prefetch().radius();
    let filtered = state.filtered_playlists();
    let sel = state.nav.sel_playlist;
    let mut out = Vec::new();
    let mut consider = |idx: usize| {
        if let Some(p) = filtered.get(idx) {
            let id = &p.data.id;
            if !state.library.tracks.contains_key(id)
                && !state.library.tracks_requested.contains(id)
            {
                out.push(id.clone());
            }
        }
    };
    consider(sel);
    for d in 1..=radius {
        if let Some(idx) = sel.checked_sub(d) {
            consider(idx);
        }
        consider(sel.saturating_add(d));
    }
    out
}

#[cfg(test)]
mod tests {
    use mineral_model::{MediaUrl, Song, SongId, SourceKind};

    use super::collect_pending_covers;
    use crate::runtime::state::{AppState, View};

    /// 造一首带封面 URL 的歌:id = `s{i}`、cover = `https://cover/{i}.jpg`。
    fn song_with_cover(i: usize) -> color_eyre::Result<Song> {
        Ok(Song::builder()
            .id(SongId::new(SourceKind::NETEASE, format!("s{i}")))
            .name(format!("song {i}"))
            .duration_ms(1000)
            .cover_url(Some(MediaUrl::remote(&format!("https://cover/{i}.jpg"))?))
            .build())
    }

    /// 收集结果里是否含某序号歌的封面 URL。
    fn collected_has(state: &AppState, i: usize) -> color_eyre::Result<bool> {
        let want = MediaUrl::remote(&format!("https://cover/{i}.jpg"))?;
        Ok(collect_pending_covers(state)
            .iter()
            .any(|(_, u)| *u == want))
    }

    /// 在播曲及其播放队列 ±[`PLAYBACK_COVER_RADIUS`] 邻居的封面进入 prefetch 集合。
    /// 刻意停在 Playlists 视图(per-view 路径只看歌单封面、此处无),隔离出在播曲这条线。
    #[test]
    fn collects_playing_track_and_queue_neighbors() -> color_eyre::Result<()> {
        let mut state = AppState::test_default()?;
        state.view.switch_to(View::Playlists);
        let queue = (0..10)
            .map(song_with_cover)
            .collect::<color_eyre::Result<Vec<Song>>>()?;
        state.playback.track = queue.get(5).cloned();
        state.player.queue = queue;

        // 在播曲 idx 5,半径 3 → idx 2..=8 应全部入集。
        for i in 2..=8 {
            assert!(
                collected_has(&state, i)?,
                "在播曲 ± playback_cover_radius(默认 3):queue[{i}] 封面应进 prefetch"
            );
        }
        // 窗口外(idx 1 / idx 9)不应入集。
        assert!(!collected_has(&state, 1)?, "窗口外 queue[1] 不应入集");
        assert!(!collected_has(&state, 9)?, "窗口外 queue[9] 不应入集");
        Ok(())
    }

    /// 在播曲即便不在队列(单首试听 / 队列已换),仍应单独保住它自己的封面。
    #[test]
    fn collects_playing_track_even_when_absent_from_queue() -> color_eyre::Result<()> {
        let mut state = AppState::test_default()?;
        state.view.switch_to(View::Playlists);
        state.player.queue = Vec::new();
        state.playback.track = Some(song_with_cover(42)?);

        assert!(collected_has(&state, 42)?, "在播曲不在队列时仍应单独入集");
        Ok(())
    }

    /// 造一个「已进 Search 布局态、搜到 1 张专辑、光标停留超防抖窗」的 state。
    fn searching_album_state() -> color_eyre::Result<AppState> {
        use std::time::{Duration, Instant};

        use mineral_channel_core::{ChannelCaps, Page};
        use mineral_model::{AlbumId, SearchKind};
        use mineral_task::{SearchPayload, TaskEvent};
        use rustc_hash::FxHashMap;

        let mut state = AppState::test_default()?;
        let mut caps = FxHashMap::default();
        caps.insert(
            SourceKind::NETEASE,
            ChannelCaps::builder()
                .searchable(vec![SearchKind::Album])
                .playlist_edit(false)
                .build(),
        );
        state.caps = caps;
        state.channel_search.enter(&state.caps);
        state.channel_search.active.set(true);
        if let Some(s) = state.channel_search.current_mut() {
            s.set_query("q");
        }
        let album = mineral_model::Album::builder()
            .id(AlbumId::new(SourceKind::NETEASE, "al1"))
            .name("al".to_owned())
            .build();
        state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Album,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Albums(vec![album]),
        });
        // 把选中时刻推到过去，越过 detail 驻留防抖窗（checked_sub 防单调时钟下溢）。
        state.nav.last_sel_change = Instant::now()
            .checked_sub(Duration::from_secs(3600))
            .unwrap_or_else(Instant::now);
        Ok(state)
    }

    /// 录提交任务的 client + disabled fetcher，驱动一次 request_detail，返回提交的任务。
    fn drive_detail(state: &mut AppState) -> color_eyre::Result<Vec<mineral_task::TaskKind>> {
        use std::sync::{Arc, Mutex};

        use crate::runtime::cover_fetch::CoverFetcher;
        use crate::test_support::TestClient;

        let submitted = Arc::new(Mutex::new(Vec::new()));
        let client = TestClient {
            submitted: Arc::clone(&submitted),
            ..TestClient::default()
        };
        super::request_detail(state, &client, &CoverFetcher::disabled());
        let tasks = submitted
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("探针锁中毒: {e}"))?
            .clone();
        Ok(tasks)
    }

    /// 驻留超窗 → 给选中专辑派 AlbumSongs。
    #[test]
    fn request_detail_dispatches_album_songs() -> color_eyre::Result<()> {
        use mineral_task::{ChannelFetchKind, TaskKind};

        let mut state = searching_album_state()?;
        let tasks = drive_detail(&mut state)?;
        assert!(
            tasks.iter().any(|t| matches!(
                t,
                TaskKind::ChannelFetch(ChannelFetchKind::AlbumDetail { .. })
            )),
            "应给选中专辑派 AlbumDetail"
        );
        Ok(())
    }

    /// 同帧第二次驱动不重复派（requested 去重）。
    #[test]
    fn request_detail_dedup_same_frame() -> color_eyre::Result<()> {
        let mut state = searching_album_state()?;
        let first = drive_detail(&mut state)?;
        assert!(!first.is_empty(), "首次应派");
        let second = drive_detail(&mut state)?;
        assert!(second.is_empty(), "同帧不重复派");
        Ok(())
    }

    /// 布局态未开 → 不派（detail 是 search 专属）。
    #[test]
    fn request_detail_skips_when_inactive() -> color_eyre::Result<()> {
        let mut state = searching_album_state()?;
        state.channel_search.active.set(false);
        let tasks = drive_detail(&mut state)?;
        assert!(tasks.is_empty(), "未进 search 布局态不派 detail");
        Ok(())
    }
}
