//! 应用全局状态。
//!
//! 阶段 3 引入左栏视图、选中索引、搜索关键字与 mock 数据;后续阶段会
//! 增加 playback / queue / cmd_mode 等。

use std::time::Instant;

use mineral_model::{Playlist, Song};

use crate::cmd::CmdMode;
use crate::components::spectrum::SpectrumState;
use crate::mock::{PlaylistKind, SongView};
use crate::playback::Playback;

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
    /// 播放状态机(stage 4 引入)。
    pub playback: Playback,
    /// 频谱状态(stage 5 引入,伪随机)。
    pub spectrum: SpectrumState,
    /// 浮动 queue 当前曲目列表(stage 6 引入)。
    pub queue: Vec<Song>,
    /// queue 浮层是否显示。
    pub queue_open: bool,
    /// queue 浮层选中行。
    pub queue_sel: usize,
    /// 当前键盘焦点。
    pub focus: Focus,
    /// 命令栏当前模式(stage 7 引入)。
    pub cmd_mode: Option<CmdMode>,
    /// 命令栏输入缓冲。
    pub cmd_buffer: String,
    /// 一条临时 hint(消息 + 失效时刻),由 tick 周期清理。
    pub hint: Option<(String, Instant)>,
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
            playback: Playback::new(),
            spectrum: SpectrumState::new(),
            queue: Vec::new(),
            queue_open: false,
            queue_sel: 0,
            focus: Focus::Left,
            cmd_mode: None,
            cmd_buffer: String::new(),
            hint: None,
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

    /// 当前可见(可能被 search 过滤)的歌单列表。
    pub fn filtered_playlists(&self) -> Vec<&PlaylistView> {
        if self.search_q.is_empty() {
            return self.playlists.iter().collect();
        }
        let q = self.search_q.to_lowercase();
        self.playlists
            .iter()
            .filter(|p| p.data.name.to_lowercase().contains(&q))
            .collect()
    }

    /// 当前可见(可能被 search 过滤)的曲目列表。
    pub fn filtered_tracks(&self) -> Vec<SongView> {
        let tracks = self.current_tracks();
        if self.search_q.is_empty() {
            return tracks;
        }
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

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
