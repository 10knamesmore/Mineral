//! 测试专用构造 helper(快照 / 渲染测试共用)。仅 `#[cfg(test)]` 编译。
//!
//! 跨 crate 复用的零件(`song` / `with_*` / `endserenading` / `chinese_football` /
//! `assert_snap!`)来自 [`mineral_test`];本模块只保留依赖 TUI 私有类型
//! (`AppState` / `SongView` / `PlaylistView`)的 fixture。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mineral_audio::AudioSnapshot;
use mineral_model::{MediaUrl, Playlist, PlaylistId, Song, SongId, SourceKind};
use mineral_protocol::{CancelFilter, PlayerSync, PlayerVersions, SongStatsWire};
use mineral_server::Client;
use mineral_task::{Priority, Snapshot, TaskEvent, TaskId, TaskKind};
use ratatui_image::picker::Picker;
use rustc_hash::FxHashMap;

use crate::app::App;
use crate::render::anim::Transition;
use crate::runtime::cover_encode::CoverEncoder;
use crate::runtime::cover_fetch::CoverFetcher;
use crate::runtime::state::{AppState, LyricExtra, View};
use crate::runtime::view_model::{PlaylistView, SongView};

// 共享零件经 mineral-test 收口;re-export 让调用点继续写 `crate::test_support::xxx`。
pub(crate) use mineral_test::{
    assert_snap, assert_snap_debug, chinese_football, endserenading, feiyu_lyrics, feiyu_song,
    qianzai_lyrics, qianzai_song, song, with_album, with_artist, with_duration, with_name,
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
pub(crate) fn state_with_playlists() -> color_eyre::Result<AppState> {
    let mut s = AppState::test_default()?;
    s.playlists = vec![
        playlist_view("p1", "EndSerenading", SourceKind::NETEASE, 10),
        playlist_view("p2", "The Power of Failing", SourceKind::NETEASE, 8),
        playlist_view("p3", "本地音乐", SourceKind::LOCAL, 5),
    ];
    Ok(s)
}

/// 在 [`state_with_playlists`] 基础上进入《EndSerenading》、填前 3 首(含收藏 /
/// 当前在播标记),view = Library,选中第 1 首。
pub(crate) fn state_with_tracks() -> color_eyre::Result<AppState> {
    let mut s = state_with_playlists()?;
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
    Ok(s)
}

/// 造一个正在播《潜在表明》、缓存了 [`mineral_test::qianzai_lyrics`] 的 `AppState`,
/// 供歌词面板 toggle / 标识快照用。`extra` 选副歌词档;`with_words` 为 false 时清掉逐字
/// (走行级 LRC 渲染路径)。position 固定 62s,落在「太陽にあぶり出される…」一行中段。
pub(crate) fn state_with_lyrics(
    extra: LyricExtra,
    with_words: bool,
) -> color_eyre::Result<AppState> {
    let mut s = AppState::test_default()?;
    let track = qianzai_song();
    let mut lyrics = qianzai_lyrics();
    if !with_words {
        // 清掉逐字时间轴,降级成行级渲染路径(保留行级时间戳与整行文本)。
        for line in &mut lyrics.original {
            if !line.kind.words().is_empty() {
                let text = line.kind.text().into_owned();
                line.kind = mineral_model::LineKind::Plain(text);
            }
        }
    }
    s.lyrics_cache.insert(track.id.clone(), lyrics);
    s.playback.track = Some(track);
    s.playback.position_ms = 62_000;
    s.lyric_extra = extra;
    Ok(s)
}

/// 造一个正在播《飞鱼转身》(只有原文 + 逐字、**无翻译 / 无罗马音**)的 `AppState`,
/// 用于验证「无副歌词可切换时,右上不显示 `[t]` 提示」这一固定行为。position 固定 165s,
/// 落在「它降落在你身旁」一行中段。
pub(crate) fn state_with_lrc_only() -> color_eyre::Result<AppState> {
    let mut s = AppState::test_default()?;
    let track = feiyu_song();
    s.lyrics_cache.insert(track.id.clone(), feiyu_lyrics());
    s.playback.track = Some(track);
    s.playback.position_ms = 165_000;
    Ok(s)
}

/// 进入「Chinese Football」歌单、填前 4 首(含最长的「不是人人都能穿十号球衣」),
/// 专用于 CJK 宽字符在多列表格里的对齐 / 截断快照。
pub(crate) fn state_with_cjk_tracks() -> color_eyre::Result<AppState> {
    let mut s = AppState::test_default()?;
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
    Ok(s)
}

/// 填 3 首**带 artist + album** 的曲目(短英文 / 长英文 / CJK 混排),专用于验证
/// Full 档 album 列「有内容」时的多列渲染 —— 其余 fixture 的 album 多为空,覆盖不到。
/// 每曲 3:30,选中第 0 首(当前在播)。
pub(crate) fn state_with_album() -> color_eyre::Result<AppState> {
    let mut s = AppState::test_default()?;
    s.playlists = vec![playlist_view("p1", "EndSerenading", SourceKind::NETEASE, 3)];
    s.view = View::Library;

    let make = |name: &str, artist: &str, album: &str| {
        with_album(
            with_artist(with_duration(with_name(song(name), name), 210_000), artist),
            album,
        )
    };
    let tracks = [
        make("Bones", "HONNE", "no song"),
        make("Location Unknown", "HONNE", "Warm on a Cold Night"),
        make("无", "草东没有派对", "丑奴儿"),
    ];

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
        .insert(PlaylistId::new(SourceKind::NETEASE, "p1"), views);
    Ok(s)
}

/// no-op [`Client`]:所有调用静默吞掉、读取类返回默认值。供测试构造 [`App`] 而不接
/// 真实 server / daemon。
#[derive(Default)]
pub(crate) struct TestClient {
    /// `request_daemon_shutdown` 调用计数(Shift+Q「退出并停止 daemon」路径断言用)。
    pub(crate) daemon_shutdowns: Arc<AtomicUsize>,
}

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
    fn player_sync(&self, _known: PlayerVersions) -> PlayerSync {
        PlayerSync::default()
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

    fn request_daemon_shutdown(&self) {
        self.daemon_shutdowns.fetch_add(1, Ordering::SeqCst);
    }
}

/// 以 defaults 配置(= 接线前硬编码常量)造一个接 [`TestClient`] + 禁用封面的裸 [`App`]。
fn test_app() -> color_eyre::Result<App> {
    test_app_with(Arc::new(TestClient::default()))
}

/// 同 [`test_app`],client 由调用方注入(需要探针 / 自定义剧本的测试用)。
fn test_app_with(client: Arc<dyn Client>) -> color_eyre::Result<App> {
    let cfg = Arc::new(mineral_config::Config::defaults()?);
    Ok(App::new(
        client,
        CoverFetcher::disabled(),
        CoverEncoder::disabled(),
        Picker::from_fontsize((8, 16)),
        /*launch_anchor*/ None,
        cfg,
        crate::runtime::ui_prefs::UiPrefs::disabled(),
    ))
}

/// 把《EndSerenading》前 `len` 首灌进 queue,当前在播设为第 `current_idx` 首。
fn fill_queue(app: &mut App, len: usize, current_idx: usize) {
    let queue = endserenading(len);
    app.state.playback.track = queue.get(current_idx).cloned();
    app.state.current = queue.get(current_idx).cloned();
    app.state.queue = queue;
}

/// 造一个接 [`TestClient`] + 禁用封面的 [`App`]:queue 填《EndSerenading》前 `len` 首,
/// 当前在播设为第 `current_idx` 首。同步构造,不需 tokio runtime。
pub(crate) fn app_with_queue(len: usize, current_idx: usize) -> color_eyre::Result<App> {
    let mut app = test_app()?;
    fill_queue(&mut app, len, current_idx);
    Ok(app)
}

/// 同 [`app_with_queue`],额外返回 [`TestClient`] 的 daemon shutdown 请求计数器
/// (Shift+Q「退出并停止 daemon」路径断言用)。
pub(crate) fn app_with_queue_probed(
    len: usize,
    current_idx: usize,
) -> color_eyre::Result<(App, Arc<AtomicUsize>)> {
    let counter = Arc::new(AtomicUsize::new(0));
    let client = TestClient {
        daemon_shutdowns: Arc::clone(&counter),
    };
    let mut app = test_app_with(Arc::new(client))?;
    fill_queue(&mut app, len, current_idx);
    Ok((app, counter))
}

/// 造一个接 [`TestClient`] + 禁用封面的 [`App`]:Library 视图,填《EndSerenading》前 `len`
/// 首到歌单 `"p1"`,选中第 `sel_track` 首(从 0 起)。同步构造,不需 tokio runtime。
pub(crate) fn app_with_library(len: usize, sel_track: usize) -> color_eyre::Result<App> {
    let mut app = test_app()?;
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
    Ok(app)
}

/// 同 [`app_with_library`],但填 `len` 首程序化生成的可区分曲目——EndSerenading
/// fixture 只有 10 首,超过一屏的滚动类测试用这个。
pub(crate) fn app_with_long_library(len: usize, sel_track: usize) -> color_eyre::Result<App> {
    let mut app = app_with_library(/*len*/ 0, /*sel_track*/ 0)?;
    let pid = PlaylistId::new(SourceKind::NETEASE, "p1");
    let views = (0..len)
        .map(|i| {
            let mut s = mineral_test::song(&format!("t{i}"));
            s.name = format!("Track {i:02}");
            SongView {
                data: s,
                loved: false,
                plays: None,
            }
        })
        .collect::<Vec<SongView>>();
    app.state.tracks_cache.insert(pid, views);
    app.state.sel_track = sel_track;
    Ok(app)
}

/// 造一个接 [`TestClient`] + 禁用封面、**已稳态进入全屏**的 [`App`]:正在播《潜在表明》、
/// 缓存逐字歌词(position 62s 落在中段),queue 填 3 首。供全屏渲染快照用。
pub(crate) fn app_in_fullscreen() -> color_eyre::Result<App> {
    let mut app = test_app()?;
    let track = qianzai_song();
    app.state
        .lyrics_cache
        .insert(track.id.clone(), qianzai_lyrics());
    app.state.playback.track = Some(track.clone());
    app.state.playback.position_ms = 62_000;
    app.state.current = Some(track);
    app.state.queue = endserenading(3);
    // 稳态全屏:fullscreen_pos 一步推到满值(step=1000)。
    let mut fs = Transition::new(1);
    fs.enter();
    fs.tick();
    app.state.fullscreen_pos = fs;
    app.state.fullscreen = true;
    Ok(app)
}
