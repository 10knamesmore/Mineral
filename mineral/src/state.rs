//! 应用全局状态。
//!
//! 阶段 3 引入左栏视图、选中索引、搜索关键字与 mock 数据;后续阶段会
//! 增加 playback / queue / cmd_mode 等。

use mineral_model::{Playlist, Song};

use crate::mock::{PlaylistKind, SongView};

/// 左栏当前展示的视图。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// 歌单列表(默认)。
    Playlists,
    /// 已选歌单内的曲目列表。
    Library,
}

/// 一条 mock 歌单 + UI 所需的额外展示字段(kind 等)。
#[derive(Clone, Debug)]
pub struct PlaylistView {
    /// 底层 model。
    pub data: Playlist,
    /// UI 展示用的歌单类别。
    pub kind: PlaylistKind,
}

impl PlaylistView {
    /// 该歌单内全部曲目时长之和(ms)。
    pub fn total_duration_ms(&self) -> u64 {
        self.data.songs.iter().map(|s| s.duration_ms).sum()
    }
}

/// 应用顶层状态。
#[allow(dead_code)] // reason: side_scroll / lib_scroll 在阶段 7 搜索过滤时会被读取
pub struct AppState {
    /// 左栏当前视图。
    pub view: View,
    /// 已加载的歌单(mock)。
    pub playlists: Vec<PlaylistView>,
    /// Playlists 视图当前选中行。
    pub sel_playlist: usize,
    /// Playlists 视图垂直滚动偏移。
    pub side_scroll: usize,
    /// Library 视图当前选中行。
    pub sel_track: usize,
    /// Library 视图垂直滚动偏移。
    pub lib_scroll: usize,
    /// 搜索关键字(stage 3 仅作占位,真实 filter 在 cmd 栏阶段实装)。
    pub search_q: String,
    /// 当前正在播放(用于 Library 视图行首 ♫ 标记)。
    pub current: Option<Song>,
}

impl AppState {
    /// 用 [`crate::mock`] 提供的 mock 数据初始化。
    pub fn new() -> Self {
        let playlists = crate::mock::fake_playlists();
        Self {
            view: View::Playlists,
            playlists,
            sel_playlist: 0,
            side_scroll: 0,
            sel_track: 0,
            lib_scroll: 0,
            search_q: String::new(),
            current: None,
        }
    }

    /// 返回当前选中歌单(Playlists 视图)的引用。
    pub fn selected_playlist(&self) -> Option<&PlaylistView> {
        self.playlists.get(self.sel_playlist)
    }

    /// 返回当前选中歌单的曲目列表 + UI 装饰(love / plays mock)。
    pub fn current_tracks(&self) -> Vec<SongView> {
        self.selected_playlist()
            .map(|p| crate::mock::decorate_songs(&p.data.songs))
            .unwrap_or_default()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
