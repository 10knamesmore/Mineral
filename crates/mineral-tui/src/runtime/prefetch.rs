//! 视口 prefetch:按 `sel ± prefetch.radius` 提前 fetch 用户即将看到的数据。
//!
//! 三件:
//! - **cover**:供主图与 Kitty 列表缩略图显示
//! - **tracks**:歌单的 length 标签在 sidebar 列表上直接可见(`—` vs 真值)
//! - **local play count**:Selected 展示当前歌曲在 Mineral 中自然播完的次数
//!
//! 封面路径只选择候选并做单批 URL 去重，请求生命周期由图片引擎管理；其余路径用各自的
//! requested 状态避免重复提交。稳态下 tick 只检查各自的预取窗口。

use crate::image::graphics::GraphicsProtocol;
use crate::runtime::backend::Backend;
use mineral_channel_core::Page;
use mineral_model::{MediaUrl, PlaylistId, Song, SongId, SourceKind};
use mineral_task::{ChannelFetchKind, Priority, TaskKind};

use crate::runtime::state::{AppState, DetailFetch, View};

/// 每 tick 调一次:封面 + 歌单 tracks + 选中歌本地完整播放次数三路 prefetch,
/// 外加聚合歌单拼贴(成员封面请求 + 就绪合成,见 [`crate::image::collage`])。
/// `queue_covers` 来自浮层的过滤视图,与 Browse 和在播候选合并后统一提交。
pub fn tick(state: &mut AppState, client: &dyn Backend, queue_covers: Vec<(SourceKind, MediaUrl)>) {
    request_covers(state, queue_covers);
    request_playlist_tracks(state, client);
    request_play_count(state, client);
    request_detail(state, client);
    request_detail_covers(state);
    crate::image::collage::tick(state);
}

/// 合并队列光标、Browse 与在播位置的封面候选,共用图片引擎的预取入口。
/// 来源随封面一起带出，决定图片引擎使用的落盘子目录。
fn request_covers(state: &mut AppState, queue_covers: Vec<(SourceKind, MediaUrl)>) {
    let items = collect_cover_candidates(state, queue_covers);
    state.images.prefetch(items);
}

/// 合并队列光标、Browse 光标与在播位置的候选,单批按 URL 去重。
///
/// 队列光标候选由浮层按过滤视图与 `prefetch.radius` 提供,优先提交;
/// Browse 使用同一列表半径,在播位置沿播放队列使用 `playback_cover_radius`。
/// 来源从条目的 id namespace 派生;缓存、失败和在途状态由图片引擎判断。
fn collect_cover_candidates(
    state: &AppState,
    queue_covers: Vec<(SourceKind, MediaUrl)>,
) -> Vec<(SourceKind, MediaUrl)> {
    let radius = *state.cfg.tui().prefetch().radius();
    let playback_radius = *state.cfg.tui().prefetch().playback_cover_radius();
    let mut out = Vec::<(SourceKind, MediaUrl)>::new();
    let push_if_new = |item: Option<(SourceKind, &MediaUrl)>,
                       out: &mut Vec<(SourceKind, MediaUrl)>| {
        if let Some((source, u)) = item
            && !out.iter().any(|(_, existing)| existing == u)
        {
            out.push((source, u.clone()));
        }
    };
    for (source, url) in queue_covers {
        push_if_new(Some((source, &url)), &mut out);
    }
    match state.browse.view.current() {
        View::Playlists => {
            // sel 是 filtered 索引,prefetch 邻居一律走 filtered,免得跟可视窗口错位。
            let filtered = state.filtered_playlists();
            let sel = state.browse.nav.playlist.sel();
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
            // 暖入口曲封面:drill 进选中歌单默认落到第 0 首,悬停期(曲目已加载)先生成
            // preview，配合渲染侧 prepare 让 drill 瞬间直接命中 kitty。只暖第 0 首；
            // remembered-pos / deep-hit 的其他入口按常规取图路径加载。
            if let Some(first) = filtered
                .get(sel)
                .and_then(|p| state.library.tracks.get(&p.data.id))
                .and_then(|tracks| tracks.first())
            {
                push_if_new(song_cover(&first.data.song), &mut out);
            }
        }
        View::Library => {
            // sel 与邻居均按过滤视图下标读取；视图借用曲目，不复制整个歌单。
            let filtered = state.filtered_tracks();
            let sel = state.browse.nav.track.sel();
            let get = |i: usize| -> Option<(SourceKind, &MediaUrl)> {
                filtered.get(i).and_then(|entry| {
                    entry
                        .data
                        .song
                        .cover_url
                        .as_ref()
                        .map(|url| (entry.data.song.id.namespace(), url))
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

/// 看 sel_playlist 周围 `prefetch.radius` 内未 cache 的歌单,提交 PlaylistDetail。
/// 只在 Playlists view 下生效 —— Library view 的当前 playlist 一定已经 cache(进 view 的前提)。
fn request_playlist_tracks(state: &mut AppState, client: &dyn Backend) {
    if state.browse.view != View::Playlists {
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

/// 查询当前选中歌曲在 Mineral 中自然播完的次数。
///
/// 本地统计无需为 API 限流，选中后立即查询，不与 remote detail 共用驻留防抖。
/// 成功值跨 selection 命中 cache；失败结果只在当前 selection 内抑制逐 tick 重试，离开
/// 后再选中会重查。`stats.level = off` 或 source 被 `exclude_sources` 排除时不查询，字段
/// 始终缺失。
fn request_play_count(state: &mut AppState, client: &dyn Backend) {
    if state.browse.view != View::Library {
        state.library.local_play_counts.leave_selection();
        return;
    }
    let Some(id) = selected_track_id(state) else {
        state.library.local_play_counts.leave_selection();
        return;
    };
    if !state.records_local_plays_for(id.namespace()) {
        state.library.local_play_counts.leave_selection();
        return;
    }
    if !state.library.local_play_counts.enter_selection(&id) {
        return;
    }
    mineral_log::debug!(target: "prefetch", song_id = id.as_str(), source = ?id.namespace(), "query local completed play count");
    client.request_song_stats(id);
}

/// search 布局态下，结果列/详情光标停留超防抖窗后，给当前 detail 栈顶帧补拉列表/详情
/// （Background 优先级），并把该实体封面搭车投给图片引擎。
///
/// 同帧只派一次（`DetailFrame.requested`）；移光标 / 下钻换新帧后可再派——失败的帧换走
/// 再回即重试（驻留窗口重新触发），与 spec「预览失败驻留重试」一致。布局态未开则不派。
fn request_detail(state: &mut AppState, client: &dyn Backend) {
    if !state.channel_search.active.on() {
        return;
    }
    let debounce =
        std::time::Duration::from_millis(*state.cfg.tui().search().channel().detail_debounce_ms());
    if state.channel_search.last_sel_change.elapsed() < debounce {
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
        state.images.prefetch([(source, url)]);
    }
}

/// 为 detail 的选中副头图与 Kitty 行缩略图预取封面，请求去重由图片引擎管理。
///
/// # Params:
///   - `state`: 当前配置、detail 列表与图片引擎
fn request_detail_covers(state: &mut AppState) {
    let items = collect_detail_cover_candidates(state, state.images.graphics_protocol());
    state.images.prefetch(items);
}

/// 驻留超过 detail 防抖窗后，按选中行优先、向两侧外扩的顺序收集封面并按 URL 去重。
/// Kitty 现读 `prefetch.radius`，只收该半径内的行；其余协议只收 artist 选中副头图。
///
/// # Params:
///   - `state`: 当前搜索布局、栈顶 detail 帧与有效配置
///   - `protocol`: 图片引擎当前生效的协议，包含终端能力限制后的结果
fn collect_detail_cover_candidates(
    state: &AppState,
    protocol: GraphicsProtocol,
) -> Vec<(SourceKind, MediaUrl)> {
    if !state.channel_search.active.on() {
        return Vec::new();
    }
    let debounce =
        std::time::Duration::from_millis(*state.cfg.tui().search().channel().detail_debounce_ms());
    if state.channel_search.last_sel_change.elapsed() < debounce {
        return Vec::new();
    }
    let Some(frame) = state
        .channel_search
        .active_results()
        .and_then(|results| results.detail.current())
    else {
        return Vec::new();
    };
    let radius = match protocol {
        GraphicsProtocol::Kitty => *state.cfg.tui().prefetch().radius(),
        _ if frame.selected_cover().is_some() => 0,
        _ => return Vec::new(),
    };
    let sel = frame.list().sel();
    let mut out = Vec::<(SourceKind, MediaUrl)>::new();
    let mut consider = |index| {
        if let Some((source, url)) = frame.row_cover(index)
            && !out.iter().any(|(_, existing)| existing == url)
        {
            out.push((source, url.clone()));
        }
    };
    consider(sel);
    for distance in 1..=radius {
        if let Some(index) = sel.checked_sub(distance) {
            consider(index);
        }
        consider(sel.saturating_add(distance));
    }
    out
}

/// 按 [`DetailFetch`] 派对应的 channel 拉取任务（artist 两路：详情 + 专辑列表；其余单路）。
pub(crate) fn submit_detail_tasks(client: &dyn Backend, fetch: DetailFetch) {
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
        .get(state.browse.nav.track.sel())
        .map(|entry| entry.data.song.id.clone())
}

/// sel 周围 `prefetch.radius` 内、既未 cache 也未请求过的歌单(sel 优先,再向两侧外扩)。
fn collect_pending_tracks(state: &AppState) -> Vec<PlaylistId> {
    let radius = *state.cfg.tui().prefetch().radius();
    let filtered = state.filtered_playlists();
    let sel = state.browse.nav.playlist.sel();
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
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use mineral_model::{
        Album, AlbumId, AlbumRef, AlbumTrack, Artist, ArtistId, MediaUrl, Playlist, PlaylistEntry,
        PlaylistId, SearchKind, Song, SongId, SourceKind,
    };
    use mineral_protocol::FinishReason;
    use mineral_task::TaskEvent;

    use super::{collect_cover_candidates, collect_detail_cover_candidates, request_play_count};
    use crate::image::graphics::GraphicsProtocol;
    use crate::runtime::state::{AppState, ArtistSection, DetailFrame, EntityRef, View};
    use crate::test_support::{TestClient, state_with_mixed_tracks};

    /// 造一首带封面 URL 的歌:id = `s{i}`、cover = `https://cover/{i}.jpg`。
    fn song_with_cover(i: usize) -> color_eyre::Result<Song> {
        Ok(Song::builder()
            .id(SongId::new(SourceKind::NETEASE, format!("s{i}")))
            .name(format!("song {i}"))
            .duration_ms(Some(1000))
            .cover_url(Some(MediaUrl::remote(&format!("https://cover/{i}.jpg"))?))
            .build())
    }

    /// Queue 过滤后的光标与在播位置共用候选集合,热改列表半径且跨路径按 URL 去重。
    #[test]
    fn queue_selection_joins_existing_cover_prefetch() -> color_eyre::Result<()> {
        use crate::components::popup::{OverlayKind, OverlayStack};
        use crate::image::ImageEngine;
        use crate::runtime::action::{Action, SelectionMove};
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut state = AppState::test_default()?;
        state.images = ImageEngine::disabled_kitty(Arc::clone(&state.cfg));
        state.player.queue = (0..8)
            .map(|index| {
                let mut song = song_with_cover(index)?;
                song.name = if index % 2 == 0 { "kept" } else { "other" }.to_owned();
                if index == 2 {
                    song.id = SongId::new(SourceKind::BILIBILI, "selected");
                }
                Ok(song)
            })
            .collect::<color_eyre::Result<Vec<_>>>()?;
        state.playback.track = state.player.queue.first().cloned();
        state.player.cursor = mineral_protocol::PlayCursor::InQueue(0);
        let mut overlays = OverlayStack::new(1);
        overlays.push(OverlayKind::queue(0));
        overlays.dispatch_key(
            &KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
            Some(Action::EnterSearch),
            &state,
        );
        for character in "kept".chars() {
            overlays.dispatch_key(
                &KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
                None,
                &state,
            );
        }
        overlays.dispatch_key(
            &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            None,
            &state,
        );
        overlays.dispatch_key(
            &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            Some(Action::MoveSelection(SelectionMove::Down(1))),
            &state,
        );
        assert_eq!(overlays.active_queue_cursor(&state), Some(2));
        for (radius, indices) in [(0, vec![2, 0]), (1, vec![2, 0, 4]), (0, vec![2, 0])] {
            let tree = mineral_config::merge_tree(
                mineral_config::default_tree()?,
                serde_json::json!({"tui": {"prefetch": {"radius": radius, "playback_cover_radius": 0}}}),
            );
            state.cfg = Arc::new(
                mineral_config::from_tree(&tree)
                    .map_err(|warning| color_eyre::eyre::eyre!("预取配置无效: {warning}"))?,
            );
            let expected = indices
                .into_iter()
                .map(|index| {
                    let song = state
                        .player
                        .queue
                        .get(index)
                        .ok_or_else(|| color_eyre::eyre::eyre!("缺少队列曲目"))?;
                    Ok((
                        song.source(),
                        song.cover_url
                            .clone()
                            .ok_or_else(|| color_eyre::eyre::eyre!("夹具应有封面"))?,
                    ))
                })
                .collect::<color_eyre::Result<Vec<_>>>()?;
            assert_eq!(
                collect_cover_candidates(&state, overlays.queue_cover_candidates(&state)),
                expected
            );
        }
        Ok(())
    }

    /// 收集结果里是否含某序号歌的封面 URL。
    fn collected_has(state: &AppState, i: usize) -> color_eyre::Result<bool> {
        let want = MediaUrl::remote(&format!("https://cover/{i}.jpg"))?;
        Ok(collect_cover_candidates(state, Vec::new())
            .iter()
            .any(|(_, u)| *u == want))
    }

    /// 造一个 Library 刚选中 Bilibili 曲目的状态。
    fn selected_bilibili_state() -> color_eyre::Result<AppState> {
        let mut state = state_with_mixed_tracks()?;
        state.browse.nav.track.set_sel(1);
        state.browse.nav.last_sel_change = Instant::now();
        Ok(state)
    }

    /// Selected 对所有 source 统一立即查询 Mineral 本地完整播放次数。
    #[test]
    fn selected_uses_local_completed_play_count_for_bilibili() -> color_eyre::Result<()> {
        let mut state = selected_bilibili_state()?;
        let queries = Arc::new(Mutex::new(Vec::new()));
        let submitted = Arc::new(Mutex::new(Vec::new()));
        let client = TestClient {
            submitted: Arc::clone(&submitted),
            song_stats_requests: Arc::clone(&queries),
            ..TestClient::default()
        };

        request_play_count(&mut state, &client);

        let id = SongId::new(SourceKind::BILIBILI, "b1");
        let recorded_queries = queries
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("统计查询探针锁中毒: {e}"))?;
        assert_eq!(recorded_queries.as_slice(), std::slice::from_ref(&id));
        assert!(
            submitted
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("任务探针锁中毒: {e}"))?
                .is_empty(),
            "本地统计不应提交 ChannelFetch::RemotePlayCount"
        );
        assert!(
            state.library.local_play_counts.has_no_cached_values(),
            "后台结果到达前不应伪造同步值"
        );

        state.apply(&TaskEvent::LocalPlayCountFetched {
            song_id: id.clone(),
            count: Some(7),
        });

        assert_eq!(state.library.local_play_counts.get(&id), Some(&7));
        Ok(())
    }

    /// 成功值跨 selection 命中 cache；失败空值只在当前 selection 内抑制重试。
    #[test]
    fn successful_result_is_cached_while_failure_retries_on_reentry() -> color_eyre::Result<()> {
        let mut state = selected_bilibili_state()?;
        let queries = Arc::new(Mutex::new(Vec::new()));
        let client = TestClient {
            song_stats_requests: Arc::clone(&queries),
            ..TestClient::default()
        };
        let bilibili = SongId::new(SourceKind::BILIBILI, "b1");
        let netease = SongId::new(SourceKind::NETEASE, "1");

        request_play_count(&mut state, &client);
        state.apply(&TaskEvent::LocalPlayCountFetched {
            song_id: bilibili.clone(),
            count: Some(2),
        });
        state.apply_track_finished(&bilibili, FinishReason::Eof);
        request_play_count(&mut state, &client);

        state.browse.nav.track.set_sel(0);
        request_play_count(&mut state, &client);

        state.browse.nav.track.set_sel(1);
        request_play_count(&mut state, &client);

        state.browse.nav.track.set_sel(0);
        request_play_count(&mut state, &client);
        state.apply(&TaskEvent::LocalPlayCountFetched {
            song_id: netease.clone(),
            count: None,
        });
        request_play_count(&mut state, &client);

        state.browse.nav.track.set_sel(1);
        request_play_count(&mut state, &client);

        state.browse.nav.track.set_sel(0);
        request_play_count(&mut state, &client);

        assert_eq!(
            *queries
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("统计查询探针锁中毒: {e}"))?,
            vec![bilibili.clone(), netease.clone(), netease]
        );
        assert_eq!(
            state
                .library
                .local_play_counts
                .get(&SongId::new(SourceKind::BILIBILI, "b1")),
            Some(&3)
        );
        Ok(())
    }

    /// 查询期间发生 eof 时丢弃旧 response，离开再进入后允许重新读取 DB 新值。
    #[test]
    fn eof_invalidates_in_flight_play_count_response() -> color_eyre::Result<()> {
        let mut state = selected_bilibili_state()?;
        let queries = Arc::new(Mutex::new(Vec::new()));
        let client = TestClient {
            song_stats_requests: Arc::clone(&queries),
            ..TestClient::default()
        };
        let id = SongId::new(SourceKind::BILIBILI, "b1");

        request_play_count(&mut state, &client);
        state.apply_track_finished(&id, FinishReason::Eof);
        state.apply(&TaskEvent::LocalPlayCountFetched {
            song_id: id.clone(),
            count: Some(2),
        });
        request_play_count(&mut state, &client);

        state.browse.view.switch_to(View::Playlists);
        request_play_count(&mut state, &client);
        state.browse.view.switch_to(View::Library);
        request_play_count(&mut state, &client);

        assert_eq!(state.library.local_play_counts.get(&id), None);
        assert_eq!(
            *queries
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("统计查询探针锁中毒: {e}"))?,
            vec![id.clone(), id]
        );
        Ok(())
    }

    /// stats.level=off 时不查询、不缓存，Selected 的本地播放次数字段保持缺失。
    #[test]
    fn selected_play_count_stays_empty_when_stats_are_off() -> color_eyre::Result<()> {
        let mut state = selected_bilibili_state()?;
        let tree = mineral_config::merge_tree(
            mineral_config::default_tree()?,
            serde_json::json!({ "stats": { "level": "off" } }),
        );
        state.cfg = Arc::new(
            mineral_config::from_tree(&tree)
                .map_err(|warning| color_eyre::eyre::eyre!("stats off 配置应合法: {warning}"))?,
        );
        let queries = Arc::new(Mutex::new(Vec::new()));
        let submitted = Arc::new(Mutex::new(Vec::new()));
        let client = TestClient {
            submitted: Arc::clone(&submitted),
            song_stats_requests: Arc::clone(&queries),
            ..TestClient::default()
        };

        request_play_count(&mut state, &client);

        assert!(
            queries
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("统计查询探针锁中毒: {e}"))?
                .is_empty()
        );
        assert!(
            submitted
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("任务探针锁中毒: {e}"))?
                .is_empty()
        );
        assert!(state.library.local_play_counts.has_no_cached_values());
        Ok(())
    }

    /// 在播曲及其播放队列 ±[`PLAYBACK_COVER_RADIUS`] 邻居的封面进入 prefetch 集合。
    /// 刻意停在 Playlists 视图(per-view 路径只看歌单封面、此处无),隔离出在播曲这条线。
    #[test]
    fn collects_playing_track_and_queue_neighbors() -> color_eyre::Result<()> {
        let mut state = AppState::test_default()?;
        state.browse.view.switch_to(View::Playlists);
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
        state.browse.view.switch_to(View::Playlists);
        state.player.queue = Vec::new();
        state.playback.track = Some(song_with_cover(42)?);

        assert!(collected_has(&state, 42)?, "在播曲不在队列时仍应单独入集");
        Ok(())
    }

    /// Playlists 视图悬停选中歌单且曲目已加载时，第 0 首入口曲封面应进 prefetch 集合，
    /// 使默认 drill 入口命中已预热的 cache。
    #[test]
    fn collects_selected_playlist_entry_track_cover() -> color_eyre::Result<()> {
        use mineral_model::{Playlist, PlaylistId};

        use crate::runtime::view_model::PlaylistView;
        use crate::test_support::entry_views;

        let mut state = AppState::test_default()?;
        state.browse.view.switch_to(View::Playlists);
        let pid = PlaylistId::new(SourceKind::NETEASE, "p1");
        state.library.playlists = vec![PlaylistView {
            data: Playlist::builder()
                .id(pid.clone())
                .name("pl".to_owned())
                .track_count(3)
                .build(),
        }];
        let songs = (0..3)
            .map(song_with_cover)
            .collect::<color_eyre::Result<Vec<Song>>>()?;
        let views = entry_views(songs);
        state.library.tracks.insert(pid, views);
        state.browse.nav.playlist.set_sel(0);

        assert!(
            collected_has(&state, 0)?,
            "入口曲(选中歌单第 0 首)封面应进 prefetch 集合"
        );
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
                .artist_sections(mineral_channel_core::ArtistSections::new(vec![
                    mineral_channel_core::ArtistSectionKind::TopSongs,
                    mineral_channel_core::ArtistSectionKind::Albums,
                ]))
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
            has_more: None,
        });
        // 把选中时刻推到过去，越过 detail 驻留防抖窗（checked_sub 防单调时钟下溢）。
        state.channel_search.last_sel_change = Instant::now()
            .checked_sub(Duration::from_secs(3600))
            .unwrap_or_else(Instant::now);
        Ok(state)
    }

    /// 有行封面的 detail 列表类型；歌曲详情展示所属专辑的曲目。
    const DETAIL_COVER_LISTS: [(SearchKind, ArtistSection); 5] = [
        (SearchKind::Playlist, ArtistSection::Hot),
        (SearchKind::Album, ArtistSection::Hot),
        (SearchKind::Song, ArtistSection::Hot),
        (SearchKind::Artist, ArtistSection::Hot),
        (SearchKind::Artist, ArtistSection::Albums),
    ];

    /// 借用预取测试中的当前 detail 帧；夹具缺帧时返回错误。
    ///
    /// # Params:
    ///   - `state`: 已进入搜索布局并载入结果的测试状态
    fn detail_cover_frame(state: &mut AppState) -> color_eyre::Result<&mut DetailFrame> {
        state
            .channel_search
            .active_results_mut()
            .and_then(|results| results.detail.current_mut())
            .ok_or_else(|| color_eyre::eyre::eyre!("封面预取夹具应有当前 detail 帧"))
    }

    /// 构造七行封面的 detail，容器来自网易云，奇数下标行来自 Bilibili，光标下标为 3。
    ///
    /// # Params:
    ///   - `kind`: detail 根实体类型
    ///   - `section`: artist 当前分区；其它实体使用 Hot
    fn detail_cover_state(
        kind: SearchKind,
        section: ArtistSection,
    ) -> color_eyre::Result<AppState> {
        let mut state = searching_album_state()?;
        let songs = (0..7)
            .map(|index| {
                let mut song = song_with_cover(index)?;
                if !index.is_multiple_of(2) {
                    song.id = SongId::new(SourceKind::BILIBILI, format!("s{index}"));
                }
                Ok(song)
            })
            .collect::<color_eyre::Result<Vec<Song>>>()?;
        let frame = detail_cover_frame(&mut state)?;
        match kind {
            SearchKind::Playlist => {
                frame.entity = EntityRef::Playlist(Box::new(
                    Playlist::builder()
                        .id(PlaylistId::new(SourceKind::NETEASE, "playlist"))
                        .name("playlist".to_owned())
                        .build(),
                ));
                frame.set_playlist_entries(PlaylistEntry::enumerate(songs));
            }
            SearchKind::Album | SearchKind::Song => {
                let mut album = Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "album"))
                    .name("album".to_owned())
                    .build();
                frame.entity = if kind == SearchKind::Album {
                    EntityRef::Album(Box::new(album.clone()))
                } else {
                    let mut song = song_with_cover(0)?;
                    song.album = Some(AlbumRef {
                        id: album.id.clone(),
                        name: album.name.clone(),
                    });
                    EntityRef::Song(Box::new(song))
                };
                album.tracks = AlbumTrack::enumerate(songs);
                frame.set_album_detail(Box::new(album));
            }
            SearchKind::Artist => {
                let mut artist = Artist::builder()
                    .id(ArtistId::new(SourceKind::NETEASE, "artist"))
                    .name("artist".to_owned())
                    .build();
                frame.entity = EntityRef::Artist(Box::new(artist.clone()));
                let albums = songs
                    .iter()
                    .enumerate()
                    .map(|(index, song)| {
                        Ok(Album::builder()
                            .id(AlbumId::new(song.id.namespace(), format!("album{index}")))
                            .name(format!("album {index}"))
                            .cover_url(Some(MediaUrl::remote(&format!(
                                "https://album-cover/{index}.jpg"
                            ))?))
                            .build())
                    })
                    .collect::<color_eyre::Result<Vec<Album>>>()?;
                artist.songs = songs;
                frame.set_artist_detail(Box::new(artist));
                frame.set_artist_albums(albums, mineral_channel_core::Page::default(), None);
            }
            SearchKind::User => color_eyre::eyre::bail!("用户结果没有 detail 曲目或专辑列表"),
        }
        frame.section = section;
        frame.list_mut().set_sel(3);
        Ok(state)
    }

    /// 从默认配置覆盖浏览预取半径，供同一状态替换配置验证热更。
    ///
    /// # Params:
    ///   - `radius`: 选中行上下各自允许预取的行数
    fn prefetch_radius_config(radius: usize) -> color_eyre::Result<Arc<mineral_config::Config>> {
        let tree = mineral_config::merge_tree(
            mineral_config::default_tree()?,
            serde_json::json!({ "tui": { "prefetch": { "radius": radius } } }),
        );
        Ok(Arc::new(mineral_config::from_tree(&tree).map_err(
            |warning| color_eyre::eyre::eyre!("预取半径配置应合法: {warning}"),
        )?))
    }

    /// 将预期行顺序转成夹具封面，保留奇数行的 Bilibili 来源。
    ///
    /// # Params:
    ///   - `indices`: 预期的行下标及提交顺序
    ///   - `section`: 热门曲与专辑使用不同 URL，避免错误分区仍命中断言
    fn expected_detail_covers(
        indices: &[usize],
        section: ArtistSection,
    ) -> color_eyre::Result<Vec<(SourceKind, MediaUrl)>> {
        let host = match section {
            ArtistSection::Hot => "cover",
            ArtistSection::Albums => "album-cover",
        };
        indices
            .iter()
            .map(|index| {
                let source = if index.is_multiple_of(2) {
                    SourceKind::NETEASE
                } else {
                    SourceKind::BILIBILI
                };
                Ok((
                    source,
                    MediaUrl::remote(&format!("https://{host}/{index}.jpg"))?,
                ))
            })
            .collect()
    }

    /// 各类 detail 都按行内来源预取；半径 0→2→0 热更立即改变范围并保留选中优先顺序。
    #[test]
    fn detail_covers_follow_live_radius_for_every_list() -> color_eyre::Result<()> {
        let selected_only = prefetch_radius_config(0)?;
        let neighbors = prefetch_radius_config(2)?;
        for (kind, section) in DETAIL_COVER_LISTS {
            let mut state = detail_cover_state(kind, section)?;
            for (cfg, indices) in [
                (&selected_only, vec![3]),
                (&neighbors, vec![3, 2, 4, 1, 5]),
                (&selected_only, vec![3]),
            ] {
                state.cfg = Arc::clone(cfg);
                assert_eq!(
                    collect_detail_cover_candidates(&state, GraphicsProtocol::Kitty),
                    expected_detail_covers(&indices, section)?,
                    "{kind:?}/{section:?} 应只预取半径内行，来源由该行 ID 决定"
                );
            }
        }
        Ok(())
    }

    /// 列表首尾不补齐另一侧的配额，未加载列表也不产生行封面候选。
    #[test]
    fn detail_covers_stop_at_list_edges() -> color_eyre::Result<()> {
        let mut state = detail_cover_state(SearchKind::Playlist, ArtistSection::Hot)?;
        state.cfg = prefetch_radius_config(2)?;
        for (sel, indices) in [(0, vec![0, 1, 2]), (6, vec![6, 5, 4])] {
            detail_cover_frame(&mut state)?.list_mut().set_sel(sel);
            assert_eq!(
                collect_detail_cover_candidates(&state, GraphicsProtocol::Kitty),
                expected_detail_covers(&indices, ArtistSection::Hot)?
            );
        }
        detail_cover_frame(&mut state)?.data = None;
        assert!(collect_detail_cover_candidates(&state, GraphicsProtocol::Kitty).is_empty());
        Ok(())
    }

    /// 非 Kitty 协议仅保留 artist 当前分区的选中副头图，来源仍取行内实体。
    #[test]
    fn detail_covers_on_other_protocols_only_prefetch_artist_selection() -> color_eyre::Result<()> {
        let cfg = prefetch_radius_config(2)?;
        for (kind, section) in DETAIL_COVER_LISTS {
            let mut state = detail_cover_state(kind, section)?;
            state.cfg = Arc::clone(&cfg);
            let expected = if kind == SearchKind::Artist {
                expected_detail_covers(&[3], section)?
            } else {
                Vec::new()
            };
            for protocol in [
                GraphicsProtocol::Halfblocks,
                GraphicsProtocol::Sixel,
                GraphicsProtocol::Iterm2,
            ] {
                assert_eq!(
                    collect_detail_cover_candidates(&state, protocol),
                    expected,
                    "{protocol:?}: {kind:?}/{section:?} 不应预取邻居缩略图"
                );
            }
        }
        Ok(())
    }

    /// Kitty 行缩略图沿用 detail 驻留防抖；退出搜索布局后不提交 detail 封面。
    #[test]
    fn detail_covers_keep_search_activation_and_dwell_gates() -> color_eyre::Result<()> {
        let mut state = detail_cover_state(SearchKind::Playlist, ArtistSection::Hot)?;
        state.cfg = prefetch_radius_config(2)?;
        assert!(!collect_detail_cover_candidates(&state, GraphicsProtocol::Kitty).is_empty());

        let settled_selection = state.channel_search.last_sel_change;
        state.channel_search.last_sel_change = Instant::now();
        assert!(collect_detail_cover_candidates(&state, GraphicsProtocol::Kitty).is_empty());

        state.channel_search.last_sel_change = settled_selection;
        state.channel_search.active.set(false);
        assert!(collect_detail_cover_candidates(&state, GraphicsProtocol::Kitty).is_empty());
        Ok(())
    }

    /// 录提交任务的 client，驱动一次 request_detail，返回提交的任务。
    fn drive_detail(state: &mut AppState) -> color_eyre::Result<Vec<mineral_task::TaskKind>> {
        use std::sync::{Arc, Mutex};

        use crate::test_support::TestClient;

        let submitted = Arc::new(Mutex::new(Vec::new()));
        let client = TestClient {
            submitted: Arc::clone(&submitted),
            ..TestClient::default()
        };
        super::request_detail(state, &client);
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

    /// 驻留窗内(光标刚动)不派——快速翻结果列不为掠过的实体打 API。
    #[test]
    fn request_detail_holds_within_dwell_window() -> color_eyre::Result<()> {
        let mut state = searching_album_state()?;
        state.channel_search.last_sel_change = std::time::Instant::now();
        let tasks = drive_detail(&mut state)?;
        assert!(tasks.is_empty(), "驻留窗内不应派 detail");
        Ok(())
    }
}
