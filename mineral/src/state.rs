//! 应用全局状态。
//!
//! mock 数据通过 [`mineral_channel_mock::MockChannel`] 提供,仅在启用
//! `mock` feature 时被加载到 [`AppState`];否则两个 cache 都是空 `Vec`,
//! UI 渲染层照样跑(只是看不到歌单)。

use std::time::Instant;

use mineral_model::Song;

use crate::cmd::CmdMode;
use crate::components::spectrum::SpectrumState;
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
    /// 已加载的歌单(从某个 channel 拉来)。
    pub playlists: Vec<PlaylistView>,
    /// 与 [`AppState::playlists`] 同序的曲目缓存(已 decorated)。
    pub tracks_cache: Vec<Vec<SongView>>,
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
    /// 命令栏当前模式。
    pub cmd_mode: Option<CmdMode>,
    /// 命令栏输入缓冲。
    pub cmd_buffer: String,
    /// 一条临时 hint(消息 + 失效时刻),由 tick 周期清理。
    pub hint: Option<(String, Instant)>,
    /// quit confirm modal 是否打开。
    pub confirm_open: bool,
}

impl AppState {
    /// 用初始数据(由可选的 mock channel 提供)构造 [`AppState`]。
    pub fn new() -> Self {
        let (playlists, tracks_cache) = load_initial_data();
        Self {
            view: View::Playlists,
            playlists,
            tracks_cache,
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
            confirm_open: false,
        }
    }

    /// 返回当前选中歌单(Playlists 视图)的引用。
    pub fn selected_playlist(&self) -> Option<&PlaylistView> {
        self.playlists.get(self.sel_playlist)
    }

    /// 返回当前选中歌单的曲目列表 + UI 装饰。
    pub fn current_tracks(&self) -> Vec<SongView> {
        self.tracks_cache
            .get(self.sel_playlist)
            .cloned()
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

/// 加载初始数据。`mock` feature 启用时从 [`mineral_channel_mock::MockChannel`]
/// 取 demo 数据,否则返回空。
#[cfg(feature = "mock")]
fn load_initial_data() -> (Vec<PlaylistView>, Vec<Vec<SongView>>) {
    use mineral_channel_mock::MockChannel;

    let ch = MockChannel::new();
    let demos = ch.demo_playlists();
    let mut playlists = Vec::with_capacity(demos.len());
    let mut tracks_cache = Vec::with_capacity(demos.len());
    for d in demos {
        playlists.push(PlaylistView {
            data: d.data.clone(),
        });
        tracks_cache.push(
            d.tracks
                .iter()
                .map(|t| SongView {
                    data: t.data.clone(),
                    loved: t.loved,
                    plays: t.plays,
                })
                .collect(),
        );
    }
    (playlists, tracks_cache)
}

#[cfg(not(feature = "mock"))]
fn load_initial_data() -> (Vec<PlaylistView>, Vec<Vec<SongView>>) {
    (Vec::new(), Vec::new())
}
