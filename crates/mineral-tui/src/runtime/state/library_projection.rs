//! 歌单曲目的用户数据装饰、延迟光标恢复与当前浏览列表投影。

use mineral_model::{PlaylistId, Song, SourceKind};

use crate::runtime::view_model::{PlaylistEntryView, PlaylistView};

use super::{AppState, LibraryQueueProjection, View, browse, track_filter};

/// 数据更新前外层歌单列表的选中身份与屏幕行，用于在新结果排序中恢复位置。
pub(super) struct PlaylistListPosition {
    /// 更新前选中的歌单。
    playlist: PlaylistId,

    /// 选中行相对视口顶部的位置。
    screen_row: usize,
}

impl AppState {
    /// 在替换歌单或更新深度搜索数据前，记录父列表选择。
    pub(super) fn playlist_list_position(&self) -> Option<PlaylistListPosition> {
        Some(PlaylistListPosition {
            playlist: self.selected_playlist_in_list()?.data.id.clone(),
            screen_row: self
                .browse
                .nav
                .playlist
                .sel()
                .saturating_sub(self.browse.nav.playlist.scroll_target()),
        })
    }

    /// 让数据更新后的父列表仍指向同一歌单；目标消失时回到列表开头。
    pub(super) fn restore_playlist_list_position(
        &mut self,
        position: Option<PlaylistListPosition>,
    ) {
        let resolved = position.and_then(|position| {
            self.filtered_playlists()
                .iter()
                .position(|p| p.data.id == position.playlist)
                .map(|index| (index, position.screen_row))
        });
        if let Some((index, screen_row)) = resolved {
            if index != self.browse.nav.playlist.sel() {
                mineral_log::debug!(target: "tui", previous_selection = self.browse.nav.playlist.sel(), selection = index, screen_row, "retain playlist selection after library update");
                self.browse.nav.playlist.place(index, screen_row);
            }
        } else {
            self.browse.nav.playlist.place(0, 0);
        }
    }

    /// 曲目到达时兑现挂起的位置恢复(进歌单时曲目还没拉到的延迟落位)。
    ///
    /// 仅当用户仍停在该歌单的 Library 视图、且光标未被动过(还在进入时的第 0 行)
    /// 才落位——不抢用户操作；目标曲目尚未加载时保留 pending。
    ///
    /// # Params:
    ///   - `id`: 刚落 cache 的歌单
    pub(super) fn apply_pending_restore(&mut self, id: &PlaylistId) {
        if self
            .browse
            .nav
            .pending_track_restore
            .as_ref()
            .is_none_or(|p| &p.playlist != id)
        {
            return;
        }
        let Some(pending) = self.browse.nav.pending_track_restore.take() else {
            return;
        };
        let still_there = self.browse.view == View::Library
            && self
                .selected_playlist()
                .is_some_and(|p| p.data.id == pending.playlist);
        if !still_there || self.browse.nav.track.sel() != 0 {
            return;
        }
        let Some(tracks) = self.library.tracks.get(&pending.playlist) else {
            return;
        };
        if !tracks.complete
            && !tracks
                .iter()
                .any(|entry| entry.data.song.id == pending.pos.song_id)
        {
            self.browse.nav.pending_track_restore = Some(pending);
            return;
        }
        let sel = pending.pos.resolve(tracks);
        // 与 activate 的即时恢复同语义:光标落位 + 按屏上相对行瞬时还原视口。
        self.browse.nav.track.place(sel, pending.pos.screen_row);
    }

    /// 给定一条 PlaylistEntry，根据当前 user-data 装饰 relation 指向的 Song。
    pub(super) fn decorate_entry(&self, entry: mineral_model::PlaylistEntry) -> PlaylistEntryView {
        let loved = self.is_liked(&entry.song);
        let plays = self.library.local_play_counts.get(&entry.song.id).copied();
        PlaylistEntryView {
            data: entry,
            loved,
            plays,
        }
    }

    /// 当前配置是否允许记录指定 source 的本地播放统计。
    ///
    /// # Params:
    ///   - `source`: 目标歌曲来源
    ///
    /// # Return:
    ///   `stats.level` 非 off 且 source 未被排除时为 `true`
    pub(crate) fn records_local_plays_for(&self, source: SourceKind) -> bool {
        *self.cfg.stats().level() != mineral_config::StatsLevel::Off
            && !self
                .cfg
                .stats()
                .exclude_sources()
                .iter()
                .any(|excluded| excluded == source.name())
    }

    /// 清空本地播放次数的查询状态与所有已装饰值。
    ///
    /// 配置热更时调用，确保切到 `stats.level = off` / 排除来源后旧值立即消失；重新启用后
    /// 下一次驻留会重新查询，不复用停采期间可能过期的缓存。
    pub(crate) fn clear_local_play_counts(&mut self) {
        self.library.local_play_counts.clear();
        for tracks in self.library.tracks.values_mut() {
            for entry in &mut tracks.entries {
                entry.plays = None;
            }
        }
    }

    /// 一首歌是否已收藏（查 `library.liked_ids` 该源的桶）。
    pub(crate) fn is_liked(&self, song: &Song) -> bool {
        self.library
            .liked_ids
            .get(&song.source())
            .is_some_and(|s| s.contains(&song.id))
    }

    /// 本地乐观切换一首歌的喜欢态(翻转 `library.liked_ids` 并重装该源曲目)。
    ///
    /// 不等 server 确认——按键即时反馈;真实持久化由 `client.toggle_love` 触发,
    /// 失败由下次 `LikedSongIdsFetched` fetch 纠正。
    ///
    /// # Params:
    ///   - `song`: 目标歌曲
    pub fn toggle_loved_local(&mut self, song: &Song) {
        let set = self.library.liked_ids.entry(song.source()).or_default();
        if !set.remove(&song.id) {
            set.insert(song.id.clone());
        }
        self.redecorate_for_source(song.source());
    }

    /// 某个 channel 的 user-data 到位 / 变化时,把 `library.tracks` 里属于该 source
    /// 的 PlaylistEntryView 全部按当前 `decorate_entry` 重建一遍。
    /// 跨 source 的歌单不动(decoration data 是 per-source 的)。
    pub(super) fn redecorate_for_source(&mut self, source: SourceKind) {
        let cache = std::mem::take(&mut self.library.tracks);
        self.library.tracks = cache
            .into_iter()
            .map(|(pid, tracks)| {
                let complete = tracks.complete;
                let next_offset = tracks.next_offset;
                let next = tracks
                    .entries
                    .into_iter()
                    .map(|sv| {
                        if sv.data.song.source() == source {
                            self.decorate_entry(sv.data)
                        } else {
                            sv
                        }
                    })
                    .collect();
                (
                    pid,
                    super::PlaylistTracks {
                        entries: next,
                        complete,
                        next_offset,
                    },
                )
            })
            .collect();
    }

    /// 构造 Browse 视图逻辑所需的只读模型借用(library + cfg);供下方 forwarder 调 BrowsePage。
    fn browse_model(&self) -> browse::BrowseModel<'_> {
        browse::BrowseModel {
            library: &self.library,
            cfg: &self.cfg,
        }
    }

    /// 当前选中歌单：外层按过滤列表光标，歌单内按打开时保存的身份。
    pub fn selected_playlist(&self) -> Option<&PlaylistView> {
        self.browse.selected_playlist(self.browse_model())
    }

    /// 外层过滤列表选中的歌单；离屏绘制也始终读取这一层的光标。
    pub fn selected_playlist_in_list(&self) -> Option<&PlaylistView> {
        self.browse.selected_playlist_in_list(self.browse_model())
    }

    /// 已打开歌单；供曲目面板在进入与返回的整个动画期间读取。
    pub fn opened_playlist(&self) -> Option<&PlaylistView> {
        self.browse.opened_playlist(self.browse_model())
    }

    /// 已打开歌单的曲目槽位(`None` = 尚未进入歌单或还没拉到)。
    pub fn current_tracks_slot(&self) -> Option<&Vec<PlaylistEntryView>> {
        self.browse.current_tracks_slot(self.browse_model())
    }

    /// 给定歌单的总时长(ms);槽位未到位时返回 0。未知时长的曲目不计入(只反映已知部分)。
    pub fn total_duration_ms_of(&self, id: &PlaylistId) -> u64 {
        self.library
            .tracks
            .get(id)
            .map(|tracks| {
                tracks
                    .iter()
                    .filter_map(|entry| entry.data.song.duration_ms)
                    .sum()
            })
            .unwrap_or(0)
    }

    /// 当前可见(被 search 过滤)的歌单列表。
    ///
    /// 空 query → 原序;非空 query → fzf 风格模糊匹配(拼音/首字母也算命中),
    /// 按 score 降序排,**stable** 保证同分按原序。
    pub fn filtered_playlists(&self) -> Vec<&PlaylistView> {
        self.browse.filtered_playlists(self.browse_model())
    }

    /// 某歌单的深度命中展示载荷(克隆一份给渲染)。空 query / 无命中返回 `None`。
    ///
    /// 调用前提:本帧已调用 [`Self::filtered_playlists`](渲染路径必然满足)，缓存已经就绪；
    /// 此访问器信任该前提，避免重复比较缓存指纹。
    pub fn deep_hit_for(&self, id: &PlaylistId) -> Option<crate::runtime::deep_search::DeepHit> {
        self.browse.deep_hit_for(id)
    }

    /// 当前过滤结果里是否存在任何深度命中。渲染端据此决定 match 列要不要占位——
    /// 全员只命中歌单名时不挤压 name 列宽。调用前提同 [`Self::deep_hit_for`]。
    pub fn has_deep_hits(&self) -> bool {
        self.browse.has_deep_hits()
    }

    /// 当前可见(被 search 过滤)的曲目列表。
    ///
    /// 命中规则:歌名 / 别名 / 任一艺人 / 专辑名取最高分作为该曲分数。
    pub(crate) fn filtered_tracks(&self) -> track_filter::FilteredTracks<'_> {
        self.browse.filtered_tracks(self.browse_model())
    }

    /// 按当前配置建立 Library 起播队列，并保留选中 occurrence。
    pub(crate) fn library_queue_projection(&self) -> Option<LibraryQueueProjection> {
        self.browse.library_queue_projection(self.browse_model())
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::SourceKind;

    use crate::test_support::playlist_view;

    use super::AppState;

    /// 合并快照整表替换:重复到达不追加不挪尾,顺序由 server 权威;
    /// 选中超界夹回 0。
    #[test]
    fn library_snapshot_replaces_whole_table() -> color_eyre::Result<()> {
        use mineral_model::{Playlist, PlaylistId};
        use mineral_task::TaskEvent;
        let pl = |id: &str, name: &str| {
            Playlist::builder()
                .id(PlaylistId::new(SourceKind::NETEASE, id))
                .name(name.to_owned())
                .build()
        };
        let mut s = AppState::test_default()?;
        s.apply(&TaskEvent::LibrarySnapshot {
            playlists: vec![pl("p1", "甲"), pl("p2", "乙")],
        });
        let names = |s: &AppState| {
            s.library
                .playlists
                .iter()
                .map(|p| p.data.name.clone())
                .collect::<Vec<String>>()
        };
        assert_eq!(names(&s), vec!["甲", "乙"]);
        s.browse.nav.playlist.set_sel(1);
        // 新快照(重排 + 藏掉一个)整表替换:无重复、无挪尾,超界选中夹回 0。
        s.apply(&TaskEvent::LibrarySnapshot {
            playlists: vec![pl("p2", "乙")],
        });
        assert_eq!(names(&s), vec!["乙"], "整表替换,不残留旧条目");
        assert_eq!(s.browse.nav.playlist.sel(), 0, "选中超界夹回");
        Ok(())
    }

    /// 首字母 query `cry` 只命中「春日影」,其它歌单淘汰。
    #[test]
    fn filtered_playlists_initials_pinyin() -> color_eyre::Result<()> {
        let mut s = AppState::test_default()?;
        s.library.playlists = vec![
            playlist_view("a", "MyGO!!!!!", SourceKind::NETEASE, 1),
            playlist_view("b", "Ave Mujica", SourceKind::NETEASE, 1),
            playlist_view("c", "春日影", SourceKind::NETEASE, 1),
        ];
        s.browse.search.playlists.set_query("cry");
        let names: Vec<&str> = s
            .filtered_playlists()
            .iter()
            .map(|p| p.data.name.as_str())
            .collect();
        assert_eq!(names, vec!["春日影"]);
        Ok(())
    }

    /// 全拼 query `chunying` 命中「春日影」(子序列覆盖 chun + ying)。
    #[test]
    fn filtered_playlists_full_pinyin() -> color_eyre::Result<()> {
        let mut s = AppState::test_default()?;
        s.library.playlists = vec![
            playlist_view("a", "春日影", SourceKind::NETEASE, 1),
            playlist_view("b", "MyGO!!!!!", SourceKind::NETEASE, 1),
        ];
        s.browse.search.playlists.set_query("chunying");
        let names: Vec<&str> = s
            .filtered_playlists()
            .iter()
            .map(|p| p.data.name.as_str())
            .collect();
        assert_eq!(names, vec!["春日影"]);
        Ok(())
    }

    /// ASCII fuzzy:`my` 命中含 m+y 子序列的项,连续命中(MyGO)排在散开(Ave Mujica)前。
    #[test]
    fn filtered_playlists_consecutive_ranks_first() -> color_eyre::Result<()> {
        let mut s = AppState::test_default()?;
        s.library.playlists = vec![
            playlist_view("a", "Ave Mujica", SourceKind::NETEASE, 1),
            playlist_view("b", "MyGO!!!!!", SourceKind::NETEASE, 1),
        ];
        s.browse.search.playlists.set_query("my");
        let names: Vec<&str> = s
            .filtered_playlists()
            .iter()
            .map(|p| p.data.name.as_str())
            .collect();
        assert_eq!(names.first().copied(), Some("MyGO!!!!!"));
        Ok(())
    }

    /// `match_for` 命中拼音/首字母时,hits 反向映射回原文 Han 字符下标。
    #[test]
    fn match_for_returns_original_indices() -> color_eyre::Result<()> {
        let mut s = AppState::test_default()?;
        s.browse.search.playlists.set_query("cry");
        let m = s
            .browse
            .search
            .playlists
            .match_for("春日影")
            .ok_or_else(|| color_eyre::eyre::eyre!("cry 应命中春日影"))?;
        assert_eq!(m.hits.as_slice(), &[0u32, 1, 2]);
        Ok(())
    }

    /// 空 query 时 `match_for` 直接返回 `None`,fast path。
    #[test]
    fn match_for_empty_query_returns_none() -> color_eyre::Result<()> {
        let s = AppState::test_default()?;
        assert!(s.browse.search.playlists.match_for("春日影").is_none());
        Ok(())
    }

    /// 挂着 pending 时曲目到达:用户仍停在该歌单且光标未动 → 按双锚补落位,
    /// pending 消费掉。
    #[test]
    fn pending_restore_lands_when_tracks_arrive() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;
        use mineral_task::TaskEvent;

        use crate::runtime::state::View;
        use crate::runtime::track_pos::{PendingRestore, TrackPos};
        use crate::test_support::{endserenading, state_with_playlists};

        let mut s = state_with_playlists()?;
        s.browse.view.switch_to(View::Library);
        s.browse.nav.playlist.set_sel(0); // p1
        s.browse.nav.track.set_sel(0);
        let pid = PlaylistId::new(mineral_model::SourceKind::NETEASE, "p1");
        s.browse.nav.opened_playlist = Some(pid.clone());
        let tracks = endserenading(5);
        let anchor = tracks
            .get(2)
            .map(|t| t.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 不足 3 首"))?;
        s.browse.nav.pending_track_restore = Some(PendingRestore {
            playlist: pid.clone(),
            pos: TrackPos {
                song_id: anchor,
                index: 2,
                screen_row: 0,
            },
        });

        let playlist = Box::new(
            mineral_model::Playlist::builder()
                .id(pid.clone())
                .name(String::new())
                .entries(mineral_model::PlaylistEntry::enumerate(tracks))
                .build(),
        );
        s.apply(&TaskEvent::PlaylistDetailFetched {
            id: pid,
            load: mineral_channel_core::PlaylistLoad::Complete,
            detail: Box::new(mineral_channel_core::PlaylistDetail::complete(*playlist)),
        });
        assert_eq!(s.browse.nav.track.sel(), 2, "曲目到达后应补落位到记忆行");
        assert!(
            s.browse.nav.pending_track_restore.is_none(),
            "pending 应被消费"
        );
        Ok(())
    }

    /// 用户在曲目到达前已自己动过光标:不抢操作,pending 静默作废。
    #[test]
    fn pending_restore_yields_to_user_movement() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;
        use mineral_task::TaskEvent;

        use crate::runtime::state::View;
        use crate::runtime::track_pos::{PendingRestore, TrackPos};
        use crate::test_support::{endserenading, state_with_playlists};

        let mut s = state_with_playlists()?;
        s.browse.view.switch_to(View::Library);
        s.browse.nav.playlist.set_sel(0);
        s.browse.nav.track.set_sel(1); // 已离开进入时的第 0 行
        let pid = PlaylistId::new(mineral_model::SourceKind::NETEASE, "p1");
        s.browse.nav.opened_playlist = Some(pid.clone());
        let tracks = endserenading(5);
        let anchor = tracks
            .get(3)
            .map(|t| t.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 不足 4 首"))?;
        s.browse.nav.pending_track_restore = Some(PendingRestore {
            playlist: pid.clone(),
            pos: TrackPos {
                song_id: anchor,
                index: 3,
                screen_row: 0,
            },
        });

        let playlist = Box::new(
            mineral_model::Playlist::builder()
                .id(pid.clone())
                .name(String::new())
                .entries(mineral_model::PlaylistEntry::enumerate(tracks))
                .build(),
        );
        s.apply(&TaskEvent::PlaylistDetailFetched {
            id: pid,
            load: mineral_channel_core::PlaylistLoad::Complete,
            detail: Box::new(mineral_channel_core::PlaylistDetail::complete(*playlist)),
        });
        assert_eq!(s.browse.nav.track.sel(), 1, "用户已动光标,不得抢落位");
        assert!(
            s.browse.nav.pending_track_restore.is_none(),
            "pending 仍应被消费"
        );
        Ok(())
    }

    /// 别的歌单先到:pending 不消费、不落位,继续等目标歌单。
    #[test]
    fn pending_restore_ignores_other_playlists() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;
        use mineral_task::TaskEvent;

        use crate::runtime::state::View;
        use crate::runtime::track_pos::{PendingRestore, TrackPos};
        use crate::test_support::{endserenading, state_with_playlists};

        let mut s = state_with_playlists()?;
        s.browse.view.switch_to(View::Library);
        s.browse.nav.playlist.set_sel(0);
        s.browse.nav.track.set_sel(0);
        let target = PlaylistId::new(mineral_model::SourceKind::NETEASE, "p1");
        let other = PlaylistId::new(mineral_model::SourceKind::NETEASE, "p2");
        s.browse.nav.opened_playlist = Some(target.clone());
        let tracks = endserenading(5);
        let anchor = tracks
            .first()
            .map(|t| t.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 为空"))?;
        s.browse.nav.pending_track_restore = Some(PendingRestore {
            playlist: target.clone(),
            pos: TrackPos {
                song_id: anchor,
                index: 0,
                screen_row: 0,
            },
        });

        let playlist = Box::new(
            mineral_model::Playlist::builder()
                .id(other.clone())
                .name(String::new())
                .entries(mineral_model::PlaylistEntry::enumerate(tracks))
                .build(),
        );
        s.apply(&TaskEvent::PlaylistDetailFetched {
            id: other,
            load: mineral_channel_core::PlaylistLoad::Complete,
            detail: Box::new(mineral_channel_core::PlaylistDetail::complete(*playlist)),
        });
        assert_eq!(s.browse.nav.track.sel(), 0);
        assert!(
            s.browse
                .nav
                .pending_track_restore
                .as_ref()
                .is_some_and(|p| p.playlist == target),
            "非目标歌单到达不应消费 pending"
        );
        Ok(())
    }
}
