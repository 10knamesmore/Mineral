//! 应用全局状态。`tracks_cache` 中的「key 不存在」== 还没拉到(渲染 "loading…")。

use std::collections::HashMap;

use mineral_model::{PlaylistId, Song};

use crate::components::spectrum::SpectrumState;
use crate::loader::LoadEvent;
use crate::playback::Playback;
use crate::view_model::{PlaylistView, SongView};

/// 左栏当前展示的视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// 歌单列表(默认)。
    Playlists,

    /// 已选歌单内的曲目列表。
    Library,
}

/// 当前键盘焦点(用于浮层与主区域之间路由按键)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    /// 主区域(左栏 / library)。
    #[default]
    Left,

    /// 浮动 queue 面板。
    Queue,
}

/// 应用顶层状态。
#[allow(dead_code)] // reason: side_scroll / lib_scroll 在阶段 7 搜索过滤时会被读取
pub struct AppState {
    /// 左栏当前视图。
    pub view: View,

    /// 已加载的歌单(跨 channel 合并;按到达顺序 append)。
    pub playlists: Vec<PlaylistView>,

    /// 歌单 id → 曲目;不在 map 里表示还没拉到。
    pub tracks_cache: HashMap<PlaylistId, Vec<SongView>>,

    /// Playlists 视图当前选中行。
    pub sel_playlist: usize,

    /// Playlists 视图垂直滚动偏移。
    pub side_scroll: usize,

    /// Library 视图当前选中行。
    pub sel_track: usize,

    /// Library 视图垂直滚动偏移。
    pub lib_scroll: usize,

    /// 搜索关键字。
    pub search_q: String,

    /// 当前正在播放(用于 Library 视图行首 ♫ 标记)。
    pub current: Option<Song>,

    /// 播放状态机。
    pub playback: Playback,

    /// 频谱状态(伪随机)。
    pub spectrum: SpectrumState,

    /// 浮动 queue 当前曲目列表。
    pub queue: Vec<Song>,

    /// queue 浮层是否显示。
    pub queue_open: bool,

    /// queue 浮层选中行。
    pub queue_sel: usize,

    /// 当前键盘焦点。
    pub focus: Focus,

    /// 是否处于搜索输入态(`/` 触发,Enter / Esc 退出)。
    pub search_mode: bool,

    /// quit confirm modal 是否打开。
    pub confirm_open: bool,
}

impl AppState {
    /// 构造空状态。所有列表 / 缓存初始为空,等 [`AppState::apply`] 增量填充。
    pub fn empty() -> Self {
        Self {
            view: View::Playlists,
            playlists: Vec::new(),
            tracks_cache: HashMap::new(),
            sel_playlist: 0,
            side_scroll: 0,
            sel_track: 0,
            lib_scroll: 0,
            search_q: String::new(),
            current: None,
            playback: Playback::new(),
            spectrum: SpectrumState::new(),
            queue: Vec::new(),
            queue_open: false,
            queue_sel: 0,
            focus: Focus::Left,
            search_mode: false,
            confirm_open: false,
        }
    }

    /// 把后台事件应用到状态。
    pub fn apply(&mut self, event: LoadEvent) {
        match event {
            LoadEvent::PlaylistsBatch(items) => {
                self.playlists
                    .extend(items.into_iter().map(|data| PlaylistView { data }));
                if self.sel_playlist >= self.playlists.len() {
                    self.sel_playlist = 0;
                }
            }
            LoadEvent::PlaylistTracks { id, tracks } => {
                let decorated = tracks
                    .into_iter()
                    .map(|data| SongView {
                        data,
                        loved: false,
                        plays: 0,
                    })
                    .collect();
                self.tracks_cache.insert(id, decorated);
            }
        }
    }

    /// 返回当前选中歌单(Playlists 视图)的引用。
    pub fn selected_playlist(&self) -> Option<&PlaylistView> {
        self.playlists.get(self.sel_playlist)
    }

    /// 当前选中歌单的曲目槽位(`None` = 还没拉到)。
    pub fn current_tracks_slot(&self) -> Option<&Vec<SongView>> {
        self.selected_playlist()
            .and_then(|p| self.tracks_cache.get(&p.data.id))
    }

    /// 当前选中歌单的曲目列表(slot 未到位时返回空)。
    pub fn current_tracks(&self) -> Vec<SongView> {
        self.current_tracks_slot().cloned().unwrap_or_default()
    }

    /// 给定歌单的总时长(ms);槽位未到位时返回 0。
    pub fn total_duration_ms_of(&self, id: &PlaylistId) -> u64 {
        self.tracks_cache
            .get(id)
            .map(|tracks| tracks.iter().map(|sv| sv.data.duration_ms).sum())
            .unwrap_or(0)
    }

    /// 当前可见(被 search 过滤)的歌单列表。
    pub fn filtered_playlists(&self) -> Vec<&PlaylistView> {
        if self.search_q.is_empty() {
            self.playlists.iter().collect()
        } else {
            let q = self.search_q.to_lowercase();
            self.playlists
                .iter()
                .filter(|p| p.data.name.to_lowercase().contains(&q))
                .collect()
        }
    }

    /// 当前可见(被 search 过滤)的曲目列表。
    pub fn filtered_tracks(&self) -> Vec<SongView> {
        let tracks = self.current_tracks();
        if self.search_q.is_empty() {
            tracks
        } else {
            let q = self.search_q.to_lowercase();
            tracks
                .into_iter()
                .filter(|sv| {
                    sv.data.name.to_lowercase().contains(&q)
                        || sv
                            .data
                            .artists
                            .first()
                            .is_some_and(|a| a.name.to_lowercase().contains(&q))
                })
                .collect()
        }
    }
}
