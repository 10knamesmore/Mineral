//! 把后台任务与数据 completion event 收敛进应用状态。

use mineral_channel_core::PlaylistLoad;
use mineral_task::TaskEvent;
use rustc_hash::FxHashSet;

use super::{AppState, CollectionTracks, LibraryUpdate};
use crate::runtime::collection::CollectionId;
use crate::runtime::view_model::{AlbumView, PlaylistView};

impl AppState {
    /// 单条事件也经过相同的批量提交边界。
    #[cfg(test)]
    pub fn apply(&mut self, event: &TaskEvent) {
        let mut update = LibraryUpdate::default();
        self.apply_task(event, &mut update);
        update.finish(self);
    }

    /// 应用业务数据；调用方处理完整批事件后调用 LibraryUpdate::finish 更新父列表。
    pub(crate) fn apply_task(&mut self, event: &TaskEvent, update: &mut LibraryUpdate) {
        match event {
            TaskEvent::LibrarySnapshot { playlists } => {
                self.ui.browse.invalidate_expansions();
                update.record(self);
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
                for old in &self.models.library.playlists {
                    let id = &old.data.id;
                    if incoming_ids.contains(id) {
                        continue;
                    }
                    has_removed_playlists = true;
                    self.models.library.tracks.remove(id);
                    self.models.library.tracks_requested.remove(id);
                    if self.ui.browse.nav.opened_collection.as_ref()
                        == Some(&CollectionId::Playlist(id.clone()))
                    {
                        self.ui.browse.nav.opened_collection = None;
                        self.ui.browse.nav.pending_track_restore = None;
                        self.ui.browse.view.switch_to(super::View::Collections);
                    }
                }
                if has_removed_playlists {
                    self.models.library.tracks_generation =
                        self.models.library.tracks_generation.next();
                }
                self.models.library.playlists = incoming_playlists;
            }
            // server 已聚合进 LibrarySnapshot,理论不会到 client。defensive:跳过。
            TaskEvent::PlaylistsFetched { .. } | TaskEvent::AlbumsFetched { .. } => {}
            // 纯埋点信号,server 记录后不转发;client 永不收到,defensive:跳过。
            TaskEvent::FetchDone { .. } => {}
            TaskEvent::PlaylistDetailFetched {
                id,
                load,
                detail,
                fresh,
            } => {
                if *fresh {
                    self.models.library.finish_playlist(id, *load, true);
                }
                if let Some(existing) = self.models.library.tracks.get(id) {
                    if existing.fresh && !*fresh {
                        return;
                    }
                    // 已确认的完整结果保留；旧的完整快照仍可被当前有效结果替换。
                    let replaces_stale_snapshot = *fresh && !existing.fresh;
                    if !replaces_stale_snapshot && existing.complete && !detail.complete {
                        return;
                    }
                    if let PlaylistLoad::More { offset } = load
                        && existing.next_offset != Some(*offset)
                    {
                        return;
                    }
                    if *load == PlaylistLoad::Preview
                        && !replaces_stale_snapshot
                        && !detail.complete
                        && existing
                            .next_offset
                            .is_some_and(|old| detail.next_offset.is_none_or(|new| old > new))
                    {
                        return;
                    }
                }
                if self.ui.browse.view == super::View::Tracks
                    && self.ui.browse.nav.opened_collection.as_ref()
                        == Some(&CollectionId::Playlist(id.clone()))
                {
                    self.ui.browse.invalidate_expansions();
                }
                update.record(self);
                let playlist = &detail.playlist;
                if *load == PlaylistLoad::Complete
                    && let Some(listed) = self
                        .models
                        .library
                        .playlists
                        .iter_mut()
                        .find(|listed| listed.data.id == *id)
                {
                    listed.data = playlist.clone();
                    listed.data.entries.clear();
                }
                let selected_index = (self.ui.browse.view == super::View::Tracks)
                    .then(|| {
                        self.selected_playlist()
                            .filter(|p| p.data.id == *id)
                            .and_then(|_| {
                                self.filtered_tracks()
                                    .get(self.ui.browse.tracks.scroll().sel())
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
                // More 的旧快照不能推进起点，否则同一请求的有效结果会被判为晚到页。
                let next_offset = match load {
                    PlaylistLoad::More { offset } if !*fresh => Some(*offset),
                    _ => detail.next_offset,
                };
                self.models.library.tracks_generation =
                    self.models.library.tracks_generation.next();
                self.models.library.tracks.insert(
                    id.clone(),
                    CollectionTracks {
                        revision: self.models.library.tracks_generation,
                        entries: decorated,
                        complete: detail.complete,
                        fresh: *fresh,
                        next_offset,
                    },
                );
                let position = selected_index.and_then(|index| {
                    self.filtered_tracks()
                        .iter()
                        .position(|entry| entry.data.index == index)
                });
                if let Some(position) = position {
                    self.ui.browse.tracks.select(position);
                }
                self.apply_pending_restore(&CollectionId::Playlist(id.clone()));
                // 搜索页保留的歌单帧按 ID 更新，切 source / kind 后也能收到曲目。
                for results in self.ui.channel_search.retained_results_mut() {
                    results.fill_playlist_entries(id, &playlist.entries, detail.complete, *fresh);
                }
            }
            TaskEvent::PlaylistDetailFailed { id, load } => {
                self.models.library.finish_playlist(id, *load, false);
            }
            TaskEvent::LikedSongIdsFetched { source, ids } => {
                self.models.library.liked_ids.insert(*source, ids.clone());
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
                if self
                    .models
                    .library
                    .local_play_counts
                    .complete(song_id, count)
                {
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
            TaskEvent::AlbumDetailFetched { id, album, fresh } => {
                if !*fresh
                    && self
                        .models
                        .library
                        .album_tracks
                        .get(id)
                        .is_some_and(|tracks| tracks.fresh)
                {
                    return;
                }
                update.record(self);
                let entries = album
                    .tracks
                    .iter()
                    .cloned()
                    .map(|entry| self.decorate_entry(entry))
                    .collect();
                self.models.library.tracks_generation =
                    self.models.library.tracks_generation.next();
                self.models.library.album_tracks.insert(
                    id.clone(),
                    CollectionTracks {
                        revision: self.models.library.tracks_generation,
                        entries,
                        complete: true,
                        fresh: *fresh,
                        next_offset: None,
                    },
                );
                if *fresh {
                    self.models.library.albums_requested.remove(id);
                }
                if let Some(listed) = self
                    .models
                    .library
                    .albums
                    .iter_mut()
                    .find(|a| a.data.id == *id)
                {
                    listed.data = (**album).clone();
                }
                self.ui.browse.invalidate_expansions();
                self.apply_pending_restore(&CollectionId::Album(id.clone()));
                self.apply_album_detail(id, album, *fresh);
            }
            TaskEvent::AlbumDetailFailed { id } => {
                self.models
                    .library
                    .fail_album(id.clone(), self.ui.browse.nav.last_sel_change);
            }
            TaskEvent::SavedAlbumsSnapshot { albums } => {
                update.record(self);
                let ids: FxHashSet<_> = albums.iter().map(|a| a.id.clone()).collect();
                self.models
                    .library
                    .album_tracks
                    .retain(|id, _| ids.contains(id));
                self.models
                    .library
                    .albums_requested
                    .retain(|id, _| ids.contains(id));
                self.models.library.albums = albums
                    .iter()
                    .map(|data| {
                        let mut data = data.clone();
                        if let Some(previous) = self
                            .models
                            .library
                            .albums
                            .iter()
                            .find(|a| a.data.id == data.id)
                        {
                            data.track_count = data.track_count.or(previous.data.track_count);
                        }
                        AlbumView { data }
                    })
                    .collect();
                if let Some(CollectionId::Album(id)) = &self.ui.browse.nav.opened_collection
                    && !ids.contains(id)
                {
                    self.ui.browse.nav.opened_collection = None;
                    self.ui.browse.nav.pending_track_restore = None;
                    self.ui.browse.view.switch_to(super::View::Collections);
                }
                self.models.library.tracks_generation =
                    self.models.library.tracks_generation.next();
                self.ui.browse.invalidate_expansions();
                mineral_log::debug!(target: "tui", albums = albums.len(), "saved album library updated");
            }
            // 写成功后的列表收敛由 server 触发的 LibrarySnapshot 完成；完成事件本身不改 AppState。
            TaskEvent::PlaylistWriteDone { .. } => {}
        }
    }
}
