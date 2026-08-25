//! 把后台任务与数据 completion event 收敛进应用状态。

use mineral_task::TaskEvent;

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
                // 合并快照整表替换:跨源顺序由 server 唯一权威(curate 出口
                // 变换后),client 不自行按源拼接。
                self.library.playlists = playlists
                    .iter()
                    .cloned()
                    .map(|data| PlaylistView { data })
                    .collect();
                if self.browse.nav.playlist.sel() >= self.library.playlists.len() {
                    self.browse.nav.playlist.set_sel(0);
                }
            }
            // server 已聚合进 LibrarySnapshot,理论不会到 client。defensive:跳过。
            TaskEvent::PlaylistsFetched { .. } => {}
            // 纯埋点信号,server 记录后不转发;client 永不收到,defensive:跳过。
            TaskEvent::FetchDone { .. } => {}
            TaskEvent::PlaylistDetailFetched { id, playlist } => {
                // 歌单详情含元信息 + 曲目;library 与 detail 都只取曲目(歌单元信息走
                // sidebar 列表那份 / detail 帧的 entity 占位)。
                let decorated = playlist
                    .entries
                    .iter()
                    .cloned()
                    .map(|data| self.decorate_entry(data))
                    .collect();
                self.library.tracks.insert(id.clone(), decorated);
                self.library.tracks_generation = self.library.tracks_generation.wrapping_add(1);
                self.apply_pending_restore(id);
                // detail 歌单帧也吃这批曲目(若当前栈顶正等它)。
                if let Some(kr) = self.channel_search.active_results_mut() {
                    kr.fill_playlist_entries(id, playlist.entries.clone());
                }
            }
            TaskEvent::LikedSongIdsFetched { source, ids } => {
                self.library.liked_ids.insert(*source, ids.clone());
                self.redecorate_for_source(*source);
            }
            // RemotePlayCount 基础设施暂时保留，但 Selected 已统一改读 Mineral 本地统计；
            // 收到旧任务 / 外部任务的结果也不能污染本地指标。
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
            TaskEvent::ArtistDetailFetched { id, artist } => self.apply_artist_detail(id, artist),
            TaskEvent::ArtistAlbumsFetched { id, albums, .. } => {
                self.apply_artist_albums(id, albums);
            }
            TaskEvent::AlbumDetailFetched { id, album } => self.apply_album_detail(id, album),
            // 歌单写操作完结由后续里程碑(歌单管理)消费;先吞掉保持 match 穷尽。
            TaskEvent::PlaylistWriteDone { .. } => {}
        }
    }
}
