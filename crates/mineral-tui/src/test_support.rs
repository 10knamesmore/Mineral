//! 测试专用构造 helper(快照 / 渲染测试共用)。仅 `#[cfg(test)]` 编译。
//!
//! 跨 crate 复用的零件(`song` / `with_*` / `endserenading` / `chinese_football` /
//! `assert_snap!`)来自 [`mineral_test`];本模块只保留依赖 TUI 私有类型
//! (`AppState` / `SongView` / `PlaylistView`)的 fixture。

use std::sync::Arc;

use mineral_audio::AudioSnapshot;
use mineral_model::{MediaUrl, Playlist, PlaylistId, Song, SongId, SourceKind};
use mineral_protocol::{CancelFilter, PlayerSnapshot, SongStatsWire};
use mineral_server::Client;
use mineral_task::{Priority, Snapshot, TaskEvent, TaskId, TaskKind};
use ratatui_image::picker::Picker;
use rustc_hash::FxHashMap;

use crate::app::App;
use crate::cover::CoverFetcher;
use crate::state::{AppState, LyricExtra, View};
use crate::view_model::{PlaylistView, SongView};

// 共享零件经 mineral-test 收口;re-export 让调用点继续写 `crate::test_support::xxx`。
pub(crate) use mineral_test::{
    assert_snap, chinese_football, endserenading, feiyu_lyrics, feiyu_song, qianzai_lyrics,
    qianzai_song, song, with_duration, with_name,
};

/// 造一个 `PlaylistView`(空曲目,只元信息)。
pub(crate) fn playlist_view(
    id: &str,
    name: &str,
    source: SourceKind,
    track_count: u64,
) -> PlaylistView {
    PlaylistView {
        data: Playlist {
            id: PlaylistId::new(source, id),
            name: name.to_owned(),
            description: String::new(),
            cover_url: None,
            track_count,
            songs: Vec::new(),
        },
    }
}

/// 造一个填了歌单的 `AppState`(view = Playlists,选中第 0 个):Mineral 两张专辑
/// + 一个本地歌单。
pub(crate) fn state_with_playlists() -> AppState {
    let mut s = AppState::empty();
    s.playlists = vec![
        playlist_view("p1", "EndSerenading", SourceKind::NETEASE, 10),
        playlist_view("p2", "The Power of Failing", SourceKind::NETEASE, 8),
        playlist_view("p3", "本地音乐", SourceKind::LOCAL, 5),
    ];
    s
}

/// 在 [`state_with_playlists`] 基础上进入《EndSerenading》、填前 3 首(含收藏 /
/// 当前在播标记),view = Library,选中第 1 首。
pub(crate) fn state_with_tracks() -> AppState {
    let mut s = state_with_playlists();
    s.view = View::Library;
    let tracks = endserenading(3);
    let plays = [1200_u32, 999, 88];
    let views = tracks
        .iter()
        .enumerate()
        .map(|(i, t)| SongView {
            data: t.clone(),
            loved: i == 1,
            plays: plays.get(i).copied(),
        })
        .collect::<Vec<SongView>>();
    s.current = tracks.first().cloned();
    s.tracks_cache
        .insert(PlaylistId::new(SourceKind::NETEASE, "p1"), views);
    s.sel_track = 1;
    s
}

/// 造一个正在播《潜在表明》、缓存了 [`mineral_test::qianzai_lyrics`] 的 `AppState`,
/// 供歌词面板 toggle / 标识快照用。`extra` 选副歌词档;`with_words` 为 false 时清掉逐字
/// (走行级 LRC 渲染路径)。position 固定 62s,落在「太陽にあぶり出される…」一行中段。
pub(crate) fn state_with_lyrics(extra: LyricExtra, with_words: bool) -> AppState {
    let mut s = AppState::empty();
    let track = qianzai_song();
    let mut lyrics = qianzai_lyrics();
    if !with_words {
        lyrics.words = mineral_model::WordLyric::default();
    }
    s.lyrics_cache.insert(track.id.clone(), lyrics);
    s.playback.track = Some(track);
    s.playback.position_ms = 62_000;
    s.lyric_extra = extra;
    s
}

/// 造一个正在播《飞鱼转身》(只有原文 + 逐字、**无翻译 / 无罗马音**)的 `AppState`,
/// 用于验证「无副歌词可切换时,右上不显示 `[t]` 提示」这一固定行为。position 固定 165s,
/// 落在「它降落在你身旁」一行中段。
pub(crate) fn state_with_lrc_only() -> AppState {
    let mut s = AppState::empty();
    let track = feiyu_song();
    s.lyrics_cache.insert(track.id.clone(), feiyu_lyrics());
    s.playback.track = Some(track);
    s.playback.position_ms = 165_000;
    s
}

/// 进入「Chinese Football」歌单、填前 4 首(含最长的「不是人人都能穿十号球衣」),
/// 专用于 CJK 宽字符在多列表格里的对齐 / 截断快照。
pub(crate) fn state_with_cjk_tracks() -> AppState {
    let mut s = AppState::empty();
    s.playlists = vec![playlist_view(
        "cf",
        "Chinese Football",
        SourceKind::NETEASE,
        10,
    )];
    s.view = View::Library;
    let tracks = chinese_football(4);
    let views = tracks
        .iter()
        .map(|t| SongView {
            data: t.clone(),
            loved: false,
            plays: None,
        })
        .collect::<Vec<SongView>>();
    s.current = tracks.first().cloned();
    s.tracks_cache
        .insert(PlaylistId::new(SourceKind::NETEASE, "cf"), views);
    s
}

/// no-op [`Client`]:所有调用静默吞掉、读取类返回默认值。供测试构造 [`App`] 而不接
/// 真实 server / daemon。
pub(crate) struct TestClient;

impl Client for TestClient {
    fn play(&self, _url: MediaUrl) {}
    fn pause(&self) {}
    fn resume(&self) {}
    fn stop(&self) {}
    fn seek(&self, _position_ms: u64) {}
    fn set_volume(&self, _pct: u8) {}
    fn audio_snapshot(&self) -> AudioSnapshot {
        AudioSnapshot::default()
    }
    fn play_song(&self, _song: Song) {}
    fn set_queue(&self, _queue: Vec<Song>, _target_id: SongId) {}
    fn cycle_play_mode(&self) {}
    fn prev_or_restart(&self) {}
    fn next_song(&self) {}
    fn player_snapshot(&self) -> PlayerSnapshot {
        PlayerSnapshot::default()
    }
    fn submit_task(&self, _kind: TaskKind, _priority: Priority) -> TaskId {
        TaskId::default()
    }
    fn cancel_tasks(&self, _filter: CancelFilter) {}
    fn drain_task_events(&self) -> Vec<TaskEvent> {
        Vec::new()
    }
    fn task_snapshot(&self) -> Snapshot {
        Snapshot {
            running: 0,
            by_lane: FxHashMap::default(),
            by_kind: FxHashMap::default(),
        }
    }
    fn pull_pcm(&self, _n: usize) -> (Vec<f32>, u32) {
        (Vec::new(), 0)
    }

    fn toggle_love(&self, _id: SongId) -> bool {
        false
    }

    fn query_song_stats(&self, _id: SongId) -> Option<SongStatsWire> {
        None
    }

    fn download(&self, _target: mineral_protocol::DownloadTarget) {}

    fn download_progress(&self) -> mineral_protocol::DownloadProgress {
        mineral_protocol::DownloadProgress::default()
    }
}

/// 造一个接 [`TestClient`] + 禁用封面的 [`App`]:queue 填《EndSerenading》前 `len` 首,
/// 当前在播设为第 `current_idx` 首。同步构造,不需 tokio runtime。
pub(crate) fn app_with_queue(len: usize, current_idx: usize) -> App {
    let mut app = App::new(
        Arc::new(TestClient),
        CoverFetcher::disabled(),
        Picker::from_fontsize((8, 16)),
        /*launch_anchor*/ None,
    );
    let queue = endserenading(len);
    app.state.playback.track = queue.get(current_idx).cloned();
    app.state.current = queue.get(current_idx).cloned();
    app.state.queue = queue;
    app
}

/// 造一个接 [`TestClient`] + 禁用封面的 [`App`]:Library 视图,填《EndSerenading》前 `len`
/// 首到歌单 `"p1"`,选中第 `sel_track` 首(从 0 起)。同步构造,不需 tokio runtime。
pub(crate) fn app_with_library(len: usize, sel_track: usize) -> App {
    let mut app = App::new(
        Arc::new(TestClient),
        CoverFetcher::disabled(),
        Picker::from_fontsize((8, 16)),
        /*launch_anchor*/ None,
    );
    let pid = PlaylistId::new(SourceKind::NETEASE, "p1");
    app.state.playlists = vec![PlaylistView {
        data: Playlist {
            id: pid.clone(),
            name: "EndSerenading".to_owned(),
            description: String::new(),
            cover_url: None,
            track_count: u64::try_from(len).unwrap_or(0),
            songs: Vec::new(),
        },
    }];
    let tracks = endserenading(len);
    let views = tracks
        .iter()
        .map(|t| SongView {
            data: t.clone(),
            loved: false,
            plays: None,
        })
        .collect::<Vec<SongView>>();
    app.state.tracks_cache.insert(pid, views);
    app.state.view = View::Library;
    app.state.sel_playlist = 0;
    app.state.sel_track = sel_track;
    app
}
