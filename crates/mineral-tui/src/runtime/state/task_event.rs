//! 把后台任务与数据 completion event 收敛进应用状态。

use mineral_channel_core::PlaylistLoad;
use mineral_task::TaskEvent;
use rustc_hash::FxHashSet;

use super::AppState;
use crate::runtime::view_model::PlaylistView;

impl AppState {
    /// 把任务或后台查询的 completion event 应用到状态。
    ///
    /// # Params:
    ///   - `event`: 本 tick 要收敛的事件
    pub fn apply(&mut self, event: &TaskEvent) {
        match event {
            TaskEvent::LibrarySnapshot { playlists } => {
                self.browse.invalidate_expansions();
                let position = self.playlist_list_position();
                let mut incoming_ids = FxHashSet::default();
                // 保留 server 的快照顺序；ID 索引只用于判断旧歌单是否仍在列表中。
                let incoming_playlists = playlists
                    .iter()
                    .map(|data| {
                        incoming_ids.insert(&data.id);
                        PlaylistView { data: data.clone() }
                    })
                    .collect::<Vec<_>>();
                let mut has_removed_playlists = false;
                for old in &self.library.playlists {
                    let id = &old.data.id;
                    if incoming_ids.contains(id) {
                        continue;
                    }
                    has_removed_playlists = true;
                    self.library.tracks.remove(id);
                    self.library.tracks_requested.remove(id);
                    if self.browse.nav.opened_playlist.as_ref() == Some(id) {
                        self.browse.nav.opened_playlist = None;
                        self.browse.nav.pending_track_restore = None;
                        self.browse.view.switch_to(super::View::Playlists);
                    }
                }
                if has_removed_playlists {
                    self.library.tracks_generation = self.library.tracks_generation.wrapping_add(1);
                }
                self.library.playlists = incoming_playlists;
                self.restore_playlist_list_position(position);
            }
            // server 已聚合进 LibrarySnapshot,理论不会到 client。defensive:跳过。
            TaskEvent::PlaylistsFetched { .. } => {}
            // 纯埋点信号,server 记录后不转发;client 永不收到,defensive:跳过。
            TaskEvent::FetchDone { .. } => {}
            TaskEvent::PlaylistDetailFetched { id, load, detail } => {
                self.library.finish_playlist(id, *load, true);
                // 完整结果到达后，晚到的预览只能收束自身请求，不能降级数据。
                if !detail.complete && self.library.playlist_complete(id) {
                    return;
                }
                if let Some(existing) = self.library.tracks.get(id) {
                    if let PlaylistLoad::More { offset } = load
                        && existing.next_offset != Some(*offset)
                    {
                        return;
                    }
                    if *load == PlaylistLoad::Preview
                        && !detail.complete
                        && existing
                            .next_offset
                            .is_some_and(|old| detail.next_offset.is_none_or(|new| old > new))
                    {
                        return;
                    }
                }
                if self.browse.view == super::View::Library
                    && self.browse.nav.opened_playlist.as_ref() == Some(id)
                {
                    self.browse.invalidate_expansions();
                }
                let parent_position = self.playlist_list_position();
                let playlist = &detail.playlist;
                let selected_index = (self.browse.view == super::View::Library)
                    .then(|| {
                        self.selected_playlist()
                            .filter(|p| p.data.id == *id)
                            .and_then(|_| {
                                self.filtered_tracks()
                                    .get(self.browse.tracks.scroll.sel())
                                    .map(|entry| entry.data.index)
                            })
                    })
                    .flatten();
                let decorated = playlist
                    .entries
                    .iter()
                    .cloned()
                    .map(|data| self.decorate_entry(data))
                    .collect();
                self.library.tracks.insert(
                    id.clone(),
                    super::PlaylistTracks {
                        entries: decorated,
                        complete: detail.complete,
                        next_offset: detail.next_offset,
                    },
                );
                self.library.tracks_generation = self.library.tracks_generation.wrapping_add(1);
                self.restore_playlist_list_position(parent_position);
                let position = selected_index.and_then(|index| {
                    self.filtered_tracks()
                        .iter()
                        .position(|entry| entry.data.index == index)
                });
                if let Some(position) = position {
                    self.browse.tracks.scroll.set_sel(position);
                }
                self.apply_pending_restore(id);
                // 搜索页保留的歌单帧按 ID 更新，切 source / kind 后也能收到曲目。
                if detail.complete {
                    for results in self.channel_search.retained_results_mut() {
                        results.fill_playlist_entries(id, &playlist.entries);
                    }
                }
            }
            TaskEvent::PlaylistDetailFailed { id, load } => {
                self.library.finish_playlist(id, *load, false);
            }
            TaskEvent::LikedSongIdsFetched { source, ids } => {
                self.library.liked_ids.insert(*source, ids.clone());
                self.redecorate_for_source(*source);
            }
            // Selected 只展示 Mineral 本地统计；远端累计播放次数不参与该投影。
            TaskEvent::RemotePlayCountFetched { .. } => {}
            TaskEvent::LocalPlayCountFetched { song_id, count } => {
                let count = if self.records_local_plays_for(song_id.namespace()) {
                    *count
                } else {
                    None
                };
                if self.library.local_play_counts.complete(song_id, count) {
                    self.redecorate_for_source(song_id.namespace());
                }
            }
            // server 已 filter,理论不会到 client。defensive:跳过。
            TaskEvent::LyricsReady { .. } => {}
            TaskEvent::SearchResults {
                source,
                kind,
                query,
                page,
                payload,
                has_more,
            } => self.apply_search_results(*source, *kind, query, *page, payload, *has_more),
            TaskEvent::SearchPageFailed {
                source,
                kind,
                query,
                page,
            } => self.apply_search_page_failed(*source, *kind, query, *page),
            TaskEvent::ArtistDetailFetched { id, artist } => self.apply_artist_detail(id, artist),
            TaskEvent::ArtistAlbumsFetched {
                id,
                albums,
                page,
                has_more,
            } => {
                self.apply_artist_albums(id, albums, *page, *has_more);
            }
            TaskEvent::ArtistAlbumsPageFailed { id, page } => {
                self.apply_artist_albums_page_failed(id, *page);
            }
            TaskEvent::AlbumDetailFetched { id, album } => self.apply_album_detail(id, album),
            // 写成功后的列表收敛由 server 触发的 LibrarySnapshot 完成；完成事件本身不改 AppState。
            TaskEvent::PlaylistWriteDone { .. } => {}
        }
    }
}
