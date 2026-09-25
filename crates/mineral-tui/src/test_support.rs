//! 测试专用构造 helper(快照 / 渲染测试共用)。仅 `#[cfg(test)]` 编译。
//!
//! 跨 crate 复用的零件(`song` / `with_*` / `endserenading` / `chinese_football` /
//! `assert_snap!`)来自 [`mineral_test`];本模块只保留依赖 TUI 私有类型
//! (`AppState` / `PlaylistEntryView` / `PlaylistView`)的 fixture。

#![cfg(test)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use mineral_channel_core::ChannelCaps;
use mineral_client::operation::Outcome;
use mineral_client::state::{PlaybackMirror, PlayerMirror, WindowTitleOverride};
use mineral_model::{
    MediaUrl, Playlist, PlaylistEntry, PlaylistId, SearchKind, Song, SongId, SourceKind,
};
use mineral_protocol::{
    DownloadSummary, Event, FailureKind, KeyContext, QueueContextWire, QueueEditOutcome, QueueOp,
    SubscriptionTopic,
};
use mineral_task::{Priority, Snapshot, TaskKind};

use crate::app::App;
use crate::image::ImageEngine;
use crate::render::anim::Toggle;
use crate::render::theme::Theme;
use crate::runtime::backend::{Backend, BackendBootstrap, Completion, CompletionQueue};
use crate::runtime::state::{AppState, LyricExtra, View};
use crate::runtime::view_model::{PlaylistEntryView, PlaylistView};

// 共享零件经 mineral-test 收口;re-export 让调用点继续写 `crate::test_support::xxx`。
pub(crate) use mineral_test::{
    assert_snap, chinese_football, endserenading, feiyu_lyrics, feiyu_song, qianzai_lyrics,
    qianzai_song, song, with_album, with_alias, with_artist, with_duration, with_name,
};

/// 从内置默认配置构造测试主题。
///
/// # Return:
///   与生产启动相同配置路径生成的主题；`default.lua` 无法加载时返回错误。
pub(crate) fn default_theme() -> color_eyre::Result<Theme> {
    let cfg = mineral_config::Config::defaults()?;
    Ok(Theme::from_config(cfg.tui().theme()))
}

/// 造一个 `PlaylistView`(空曲目,只元信息)。
pub(crate) fn playlist_view(
    id: &str,
    name: &str,
    source: SourceKind,
    track_count: u64,
) -> PlaylistView {
    PlaylistView {
        data: Playlist::builder()
            .id(PlaylistId::new(source, id))
            .name(name.to_owned())
            .track_count(track_count)
            .build(),
    }
}

/// 混源三曲(netease / bilibili / local 各一首)——聚合收藏类混源列表的测试原料。
pub(crate) fn mixed_source_songs() -> Vec<Song> {
    let netease = endserenading(1)
        .into_iter()
        .next()
        .unwrap_or_else(|| song("n1"));
    let mut bilibili = with_artist(with_name(song("b1"), "夜間飛行"), "Chinese Football");
    bilibili.id = SongId::new(SourceKind::BILIBILI, "b1");
    let mut local = with_name(song("l1"), "Local Rip");
    local.id = SongId::new(SourceKind::LOCAL, "l1");
    vec![netease, bilibili, local]
}

/// 按输入顺序枚举 0-based PlaylistEntry，并包成无装饰 PlaylistEntryView。
pub(crate) fn entry_views(songs: Vec<Song>) -> Vec<PlaylistEntryView> {
    PlaylistEntry::enumerate(songs)
        .into_iter()
        .map(|data| PlaylistEntryView {
            data,
            loved: false,
            plays: None,
        })
        .collect()
}

/// 造一个选中混源歌单(source = mineral 的聚合收藏)的 `AppState`,view = Library,
/// 曲目为 [`mixed_source_songs`]，无在播曲。
pub(crate) fn state_with_mixed_tracks() -> color_eyre::Result<AppState> {
    let mut s = AppState::test_default()?;
    let pid = PlaylistId::new(SourceKind::MINERAL, "favorites");
    s.library.playlists = vec![playlist_view(
        "favorites",
        "Favorites",
        SourceKind::MINERAL,
        3,
    )];
    s.browse.nav.opened_playlist = Some(pid.clone());
    s.browse.view.switch_to(View::Library);
    let views = entry_views(mixed_source_songs())
        .into_iter()
        .map(|entry| PlaylistEntryView {
            loved: true,
            ..entry
        })
        .collect();
    s.library.tracks.insert(
        pid,
        crate::runtime::state::PlaylistTracks {
            entries: views,
            complete: true,
            next_offset: None,
        },
    );
    Ok(s)
}

/// 造一个填了歌单的 `AppState`(view = Playlists,选中第 0 个):Mineral 两张专辑
/// + 一个本地歌单。
pub(crate) fn state_with_playlists() -> color_eyre::Result<AppState> {
    let mut s = AppState::test_default()?;
    s.library.playlists = vec![
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
    s.browse.nav.opened_playlist = Some(PlaylistId::new(SourceKind::NETEASE, "p1"));
    s.browse.view.switch_to(View::Library);
    let tracks = endserenading(3);
    let plays = [1200_u32, 999, 88];
    let views = PlaylistEntry::enumerate(tracks.clone())
        .into_iter()
        .enumerate()
        .map(|(i, data)| PlaylistEntryView {
            data,
            loved: i == 1,
            plays: plays.get(i).copied(),
        })
        .collect();
    s.player.current = tracks.first().cloned();
    s.library.tracks.insert(
        PlaylistId::new(SourceKind::NETEASE, "p1"),
        crate::runtime::state::PlaylistTracks {
            entries: views,
            complete: true,
            next_offset: None,
        },
    );
    s.browse.nav.track.set_sel(1);
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
        for line in &mut lyrics.lines {
            if !line.kind.words().is_empty() {
                let text = line.kind.text().into_owned();
                line.kind = mineral_model::LineKind::Plain(text);
            }
        }
    }
    s.library.lyrics.insert(track.id.clone(), lyrics);
    s.playback.track = Some(track);
    s.playback.position_ms = 62_000;
    s.browse.lyric_view.extra = extra;
    Ok(s)
}

/// 造一个正在播《飞鱼转身》(只有原文 + 逐字、**无翻译 / 无罗马音**)的 `AppState`,
/// 用于验证「无副歌词可切换时,右上不显示 `[t]` 提示」这一固定行为。position 固定 165s,
/// 落在「它降落在你身旁」一行中段。
pub(crate) fn state_with_lrc_only() -> color_eyre::Result<AppState> {
    let mut s = AppState::test_default()?;
    let track = feiyu_song();
    s.library.lyrics.insert(track.id.clone(), feiyu_lyrics());
    s.playback.track = Some(track);
    s.playback.position_ms = 165_000;
    Ok(s)
}

/// 进入「Chinese Football」歌单、填前 4 首(含最长的「不是人人都能穿十号球衣」),
/// 专用于 CJK 宽字符在多列表格里的对齐 / 截断快照。
pub(crate) fn state_with_cjk_tracks() -> color_eyre::Result<AppState> {
    let mut s = AppState::test_default()?;
    s.library.playlists = vec![playlist_view(
        "cf",
        "Chinese Football",
        SourceKind::NETEASE,
        10,
    )];
    s.browse.nav.opened_playlist = Some(PlaylistId::new(SourceKind::NETEASE, "cf"));
    s.browse.view.switch_to(View::Library);
    let tracks = chinese_football(4);
    let views = entry_views(tracks.clone());
    s.player.current = tracks.first().cloned();
    s.library.tracks.insert(
        PlaylistId::new(SourceKind::NETEASE, "cf"),
        crate::runtime::state::PlaylistTracks {
            entries: views,
            complete: true,
            next_offset: None,
        },
    );
    Ok(s)
}

/// 填 3 首**带 artist + album** 的曲目(短英文 / 长英文 / CJK 混排),专用于验证
/// Full 档 album 列「有内容」时的多列渲染 —— 其余 fixture 的 album 多为空,覆盖不到。
/// 每曲 3:30,选中第 0 首(当前在播)。
pub(crate) fn state_with_album() -> color_eyre::Result<AppState> {
    let mut s = AppState::test_default()?;
    s.library.playlists = vec![playlist_view("p1", "EndSerenading", SourceKind::NETEASE, 3)];
    s.browse.nav.opened_playlist = Some(PlaylistId::new(SourceKind::NETEASE, "p1"));
    s.browse.view.switch_to(View::Library);

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

    let views = entry_views(tracks.to_vec());
    s.player.current = tracks.first().cloned();
    s.library.tracks.insert(
        PlaylistId::new(SourceKind::NETEASE, "p1"),
        crate::runtime::state::PlaylistTracks {
            entries: views,
            complete: true,
            next_offset: None,
        },
    );
    Ok(s)
}

/// 进程内测试后端:读取测试注入的镜像,按操作记录探针或投递预设结论。
/// 未模拟的操作为空操作;构造 [`App`] 时无需连接 daemon。
#[derive(Default)]
pub(crate) struct TestClient {
    /// `request_daemon_shutdown` 调用计数(Shift+Q「退出并停止 daemon」路径断言用)。
    pub(crate) daemon_shutdowns: Arc<AtomicUsize>,

    /// `submit_task` 收到的任务序列(深度搜索全量补拉等提交路径断言用)。
    pub(crate) submitted: Arc<Mutex<Vec<TaskKind>>>,

    /// `request_song_stats` 收到的歌曲 ID 序列(Selected 本地播放次数查询断言用)。
    pub(crate) song_stats_requests: Arc<Mutex<Vec<SongId>>>,

    /// `toggle_love` 收到的歌曲序列，用于核对喜欢态持久化请求的目标。
    pub(crate) love_requests: Arc<Mutex<Vec<Song>>>,

    /// 队列操作记录 `(操作名, 歌 id 全限定串)`(操作菜单的插播/追加路径断言用)。
    pub(crate) queue_ops: QueueOpsLog,

    /// 每次队列请求的语境记录 `(操作名, 队列语境)`；批量请求只记录一次。
    pub(crate) queue_contexts: QueueContextLog,

    /// `render_copy_template` 收到的模板下标记录(恒回 `Err`,避免测试真碰系统剪贴板)。
    pub(crate) copy_template_calls: Arc<Mutex<Vec<usize>>>,

    /// `queue_edit` 收到的编辑操作序列(队列面板的删除 / 移动 / 清理路径断言用)。
    pub(crate) queue_edits: QueueEditLog,

    /// `seek` 收到的目标位置(ms)序列(全屏歌词 Enter 跳到焦点行的绝对 seek 路径断言用)。
    pub(crate) seeks: Arc<Mutex<Vec<u64>>>,

    /// `set_volume` 收到的目标音量序列(音量键路径断言用)。
    pub(crate) volumes: Arc<Mutex<Vec<u8>>>,

    /// 播放控制调用序列，用于确认唤出提示的首次按键仍会发送命令。
    pub(crate) playback_controls: Arc<Mutex<Vec<&'static str>>>,

    /// 完成事件队列(测试可注入结论)。
    pub(crate) completions: Arc<CompletionQueue>,

    /// 播放镜像(默认空;测试可直接改)。
    pub(crate) player: PlayerMirror,

    /// 播放锚点镜像。
    pub(crate) playback: PlaybackMirror,

    /// 任务摘要。
    pub(crate) tasks: Option<Snapshot>,

    /// 下载摘要。
    pub(crate) downloads_summary: DownloadSummary,

    /// 待消费事件。
    pub(crate) events: Arc<Mutex<Vec<Event>>>,

    /// PCM 样本。
    pub(crate) pcm: Arc<Mutex<Vec<f32>>>,

    /// PCM 断续标记。
    pub(crate) pcm_discontinuity: Arc<std::sync::atomic::AtomicBool>,
}

/// [`TestClient::queue_ops`] 的记录容器:`(操作名, 歌 id 全限定串)` 序列。
pub(crate) type QueueOpsLog = Arc<Mutex<Vec<(&'static str, String)>>>;

/// [`TestClient::queue_contexts`] 的记录容器:`(操作名, 队列语境)` 序列。
pub(crate) type QueueContextLog =
    Arc<Mutex<Vec<(&'static str, mineral_protocol::QueueContextWire)>>>;

/// [`TestClient::queue_edits`] 的记录容器:编辑操作序列。
pub(crate) type QueueEditLog = Arc<Mutex<Vec<mineral_protocol::QueueOp>>>;

impl Backend for TestClient {
    fn bootstrap(&self) -> BackendBootstrap {
        BackendBootstrap::default()
    }

    fn completions(&self) -> &Arc<CompletionQueue> {
        &self.completions
    }

    fn refresh_script_binds(&self) {}

    fn connected(&self) -> bool {
        true
    }

    fn drain_events(&self) -> Vec<Event> {
        self.events
            .lock()
            .map(|mut guard| guard.drain(..).collect())
            .unwrap_or_default()
    }

    fn events_dropped(&self) -> u64 {
        0
    }

    fn with_player(&self, f: &mut dyn FnMut(&PlayerMirror)) {
        f(&self.player);
    }

    fn with_playback(&self, f: &mut dyn FnMut(&PlaybackMirror)) {
        f(&self.playback);
    }

    fn tasks(&self) -> Option<Snapshot> {
        self.tasks.clone()
    }

    fn downloads_summary(&self) -> DownloadSummary {
        self.downloads_summary.clone()
    }

    fn downloads_detail(&self) -> Option<mineral_client::state::DownloadsDetailMirror> {
        None
    }

    fn window_title_override(&self) -> WindowTitleOverride {
        WindowTitleOverride::NotKnown
    }

    fn subscribe(&self, _topic: SubscriptionTopic) {}

    fn unsubscribe(&self, _topic: SubscriptionTopic) {}

    fn drain_pcm(&self) -> Vec<f32> {
        self.pcm
            .lock()
            .map(|mut guard| guard.drain(..).collect())
            .unwrap_or_default()
    }

    fn take_pcm_discontinuity(&self) -> bool {
        self.pcm_discontinuity.swap(false, Ordering::SeqCst)
    }

    fn pause(&self) {
        if let Ok(mut calls) = self.playback_controls.lock() {
            calls.push("pause");
        }
    }

    fn resume(&self) {
        if let Ok(mut calls) = self.playback_controls.lock() {
            calls.push("resume");
        }
    }

    fn seek(&self, position_ms: u64) {
        if let Ok(mut v) = self.seeks.lock() {
            v.push(position_ms);
        }
    }

    fn set_volume(&self, pct: u8) {
        if let Ok(mut v) = self.volumes.lock() {
            v.push(pct);
        }
    }

    fn cycle_play_mode(&self) {
        if let Ok(mut calls) = self.playback_controls.lock() {
            calls.push("cycle_play_mode");
        }
    }

    fn prev_or_restart(&self) {
        if let Ok(mut calls) = self.playback_controls.lock() {
            calls.push("prev_or_restart");
        }
    }

    fn next_song(&self) {
        if let Ok(mut calls) = self.playback_controls.lock() {
            calls.push("next_song");
        }
    }

    fn play_song(&self, song: Song) {
        if let Ok(mut v) = self.queue_ops.lock() {
            v.push(("play_song", song.id.qualified()));
        }
    }

    fn play_queue(&self, songs: Vec<Song>, target: usize, context: QueueContextWire) {
        let len = songs.len();
        let outcome = match songs.get(target) {
            Some(target_song) => {
                if let Ok(mut v) = self.queue_ops.lock() {
                    v.push((
                        "play_queue",
                        format!("{len}:{target}:{}", target_song.id.qualified()),
                    ));
                }
                if let Ok(mut v) = self.queue_contexts.lock() {
                    v.push(("play_queue", context));
                }
                Outcome::Applied(())
            }
            None if songs.is_empty() => Outcome::Failed {
                kind: FailureKind::Invalid,
                detail: mineral_protocol::PlayQueueError::Empty.to_string(),
            },
            None => Outcome::Failed {
                kind: FailureKind::Invalid,
                detail: mineral_protocol::PlayQueueError::TargetOutOfBounds { target, len }
                    .to_string(),
            },
        };
        self.completions.push(Completion::PlayQueue(outcome));
    }

    fn queue_insert_next(&self, songs: Vec<Song>, context: QueueContextWire) {
        if let Ok(mut v) = self.queue_ops.lock() {
            v.extend(
                songs
                    .iter()
                    .map(|song| ("insert_next", song.id.qualified())),
            );
        }
        if let Ok(mut v) = self.queue_contexts.lock() {
            v.push(("insert_next", context));
        }
    }

    fn queue_append(&self, songs: Vec<Song>, context: QueueContextWire) {
        if let Ok(mut v) = self.queue_ops.lock() {
            v.extend(songs.iter().map(|song| ("append", song.id.qualified())));
        }
        if let Ok(mut v) = self.queue_contexts.lock() {
            v.push(("append", context));
        }
    }

    fn queue_edit(&self, op: QueueOp) {
        if let Ok(mut v) = self.queue_edits.lock() {
            v.push(op);
        }
        self.completions
            .push(Completion::QueueEdit(Outcome::Applied(
                QueueEditOutcome::Applied,
            )));
    }

    fn submit_task(&self, kind: TaskKind, _priority: Priority) {
        if let Ok(mut v) = self.submitted.lock() {
            v.push(kind);
        }
    }

    // 测试后端立即接收任务，没有待提交积压。
    fn flush_task_submissions(&self) {}

    fn prioritize_task(&self, _kind: &TaskKind) {}

    fn pending_task_count(&self) -> usize {
        0
    }

    fn download(&self, _target: mineral_protocol::DownloadTarget) {}

    fn stop_download(&self, _id: mineral_protocol::DownloadId) {
        self.completions
            .push(Completion::StopDownload(Outcome::Applied(())));
    }

    fn toggle_love(&self, song: Song) {
        if let Ok(mut requests) = self.love_requests.lock() {
            requests.push(song.clone());
        }
        self.completions.push(Completion::Love {
            song_id: song.id.clone(),
            outcome: Outcome::Applied(false),
        });
    }

    fn request_song_stats(&self, id: SongId) {
        if let Ok(mut requests) = self.song_stats_requests.lock() {
            requests.push(id);
        }
    }

    fn invoke_action(&self, _name: &str, _ctx: Option<KeyContext>) {}

    fn render_copy_template(&self, index: usize, _ctx: mineral_protocol::CopyTemplateCtx) {
        if let Ok(mut v) = self.copy_template_calls.lock() {
            v.push(index);
        }
        self.completions
            .push(Completion::CopyTemplate(Outcome::Applied(Err(
                "test stub".to_owned()
            ))));
    }

    fn report_terminal_state(&self, _rows: u16, _cols: u16, _fullscreen: bool, _focused: bool) {}

    fn request_daemon_shutdown(&self) {
        self.daemon_shutdowns.fetch_add(1, Ordering::SeqCst);
    }
}

/// 以默认配置构造接入 [`TestClient`] 的 [`App`],不启动图片 worker。
fn test_app() -> color_eyre::Result<App> {
    test_app_with(Arc::new(TestClient::default()))
}

/// 同 [`test_app`],client 由调用方注入(需要探针 / 自定义剧本的测试用)。
fn test_app_with(client: Arc<dyn Backend>) -> color_eyre::Result<App> {
    let cfg = Arc::new(mineral_config::Config::defaults()?);
    let images = ImageEngine::disabled(Arc::clone(&cfg));
    Ok(App::new(
        client,
        images,
        /*launch_anchor*/ None,
        cfg,
        crate::runtime::ui::prefs::UiPrefs::disabled(),
    ))
}

/// 把《EndSerenading》前 `len` 首灌进 queue,当前在播设为第 `current_idx` 首。
fn fill_queue(app: &mut App, len: usize, current_idx: usize) {
    let queue = endserenading(len);
    app.state.playback.track = queue.get(current_idx).cloned();
    app.state.player.current = queue.get(current_idx).cloned();
    app.state.player.queue = queue;
}

/// 造一个接 [`TestClient`] 且不启动图片 worker 的 [`App`]:queue 填《EndSerenading》前 `len` 首,
/// 当前在播设为第 `current_idx` 首。同步构造,不需 tokio runtime。
pub(crate) fn app_with_queue(len: usize, current_idx: usize) -> color_eyre::Result<App> {
    let mut app = test_app()?;
    fill_queue(&mut app, len, current_idx);
    Ok(app)
}

/// 同 [`app_with_queue`],额外返回 `set_volume` 收到的目标音量序列(音量键路径断言用)。
pub(crate) fn app_with_queue_volume_probed(
    len: usize,
    current_idx: usize,
) -> color_eyre::Result<(App, Arc<Mutex<Vec<u8>>>)> {
    let volumes: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let client = TestClient {
        volumes: Arc::clone(&volumes),
        ..TestClient::default()
    };
    let mut app = test_app_with(Arc::new(client))?;
    fill_queue(&mut app, len, current_idx);
    Ok((app, volumes))
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
        ..TestClient::default()
    };
    let mut app = test_app_with(Arc::new(client))?;
    fill_queue(&mut app, len, current_idx);
    Ok((app, counter))
}

/// 同 [`app_with_queue`],额外返回 [`TestClient`] 的队列编辑记录
/// (queue 面板的删除 / 移动 / 清理路径断言用)。
pub(crate) fn app_with_queue_edits(
    len: usize,
    current_idx: usize,
) -> color_eyre::Result<(App, QueueEditLog)> {
    let edits: QueueEditLog = Arc::new(Mutex::new(Vec::new()));
    let client = TestClient {
        queue_edits: Arc::clone(&edits),
        ..TestClient::default()
    };
    let mut app = test_app_with(Arc::new(client))?;
    fill_queue(&mut app, len, current_idx);
    Ok((app, edits))
}

/// 造一个停在 Playlists 视图、填 [`state_with_playlists`] 同款三歌单(曲目均未缓存)
/// 的 [`App`],额外返回 [`TestClient`] 的 submit_task 任务记录(深度搜索补拉断言用)。
pub(crate) fn app_with_playlists_probed() -> color_eyre::Result<(App, Arc<Mutex<Vec<TaskKind>>>)> {
    let submitted = Arc::new(Mutex::new(Vec::new()));
    let client = TestClient {
        submitted: Arc::clone(&submitted),
        ..TestClient::default()
    };
    let mut app = test_app_with(Arc::new(client))?;
    app.state.library.playlists = vec![
        playlist_view("p1", "EndSerenading", SourceKind::NETEASE, 10),
        playlist_view("p2", "The Power of Failing", SourceKind::NETEASE, 8),
        playlist_view("p3", "本地音乐", SourceKind::LOCAL, 5),
    ];
    app.state.browse.view.switch_to(View::Playlists);
    Ok((app, submitted))
}

/// 造一个接 [`TestClient`] 且不启动图片 worker 的 [`App`]:Library 视图,填《EndSerenading》前 `len`
/// 首到歌单 `"p1"`,选中第 `sel_track` 首(从 0 起)。同步构造,不需 tokio runtime。
pub(crate) fn app_with_library(len: usize, sel_track: usize) -> color_eyre::Result<App> {
    let mut app = test_app()?;
    let pid = PlaylistId::new(SourceKind::NETEASE, "p1");
    app.state.browse.nav.opened_playlist = Some(pid.clone());
    app.state.library.playlists = vec![PlaylistView {
        data: Playlist::builder()
            .id(pid.clone())
            .name("EndSerenading".to_owned())
            .track_count(u64::try_from(len).unwrap_or(0))
            .build(),
    }];
    let tracks = endserenading(len);
    let views = entry_views(tracks);
    app.state.library.tracks.insert(
        pid,
        crate::runtime::state::PlaylistTracks {
            entries: views,
            complete: true,
            next_offset: None,
        },
    );
    app.state.browse.view.switch_to(View::Library);
    while !app.state.browse.view.at_max() {
        app.state.browse.view.tick();
    }
    app.state.browse.nav.playlist.set_sel(0);
    app.state.browse.nav.track.set_sel(sel_track);
    Ok(app)
}

/// 造「page morph(Browse ↔ Search)中途」的 [`App`]:Library 选中曲带封面 A(纯品红),
/// search 端 album 结果已成 detail 根帧、头图 B(纯青);`cache_*` 控制两端图是否入
/// 图片解码缓存，覆盖双图合成、单图独飞与 preview 未就绪时留空。morph 停在 4/8 拍中途。
pub(crate) fn app_in_search_morph(
    cache_browse: bool,
    cache_detail: bool,
) -> color_eyre::Result<App> {
    use mineral_channel_core::Page;
    use mineral_model::{Album, AlbumId};
    use mineral_task::{SearchPayload, TaskEvent};

    let mut app = app_with_library(/*len*/ 3, /*sel_track*/ 0)?;
    let url_a = MediaUrl::remote("https://x.y/browse-a.jpg")?;
    let pid = PlaylistId::new(SourceKind::NETEASE, "p1");
    if let Some(sv) = app
        .state
        .library
        .tracks
        .get_mut(&pid)
        .and_then(|views| views.get_mut(0))
    {
        sv.data.song.cover_url = Some(url_a.clone());
    }
    if cache_browse {
        app.state
            .images
            .cache
            .insert_test(&url_a, Arc::new(solid_cover(255, 0, 255)));
    }
    let url_b = MediaUrl::remote("https://x.y/detail-b.jpg")?;
    app.state.caps.insert(
        SourceKind::NETEASE,
        ChannelCaps::builder()
            .searchable(vec![SearchKind::Album])
            .playlist_edit(false)
            .artist_sections(mineral_channel_core::ArtistSections::new(vec![]))
            .build(),
    );
    app.state.channel_search.enter(&app.state.caps);
    if let Some(session) = app.state.channel_search.current_mut() {
        session.set_query("q");
    }
    app.state.apply(&TaskEvent::SearchResults {
        source: SourceKind::NETEASE,
        kind: SearchKind::Album,
        query: "q".to_owned(),
        page: Page::default(),
        payload: SearchPayload::Albums(vec![
            Album::builder()
                .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                .name("Aurora".to_owned())
                .cover_url(Some(url_b.clone()))
                .build(),
        ]),
        has_more: None,
    });
    if cache_detail {
        app.state
            .images
            .cache
            .insert_test(&url_b, Arc::new(solid_cover(0, 255, 255)));
    }
    let mut active = Toggle::new(8);
    active.set(true);
    for _ in 0..4 {
        active.tick();
    }
    app.state.channel_search.active = active;
    Ok(app)
}

/// 纯色 64×64 测试封面(选 UI 不会出现的像素色作探针,渲染断言据此扫 buffer)。
pub(crate) fn solid_cover(r: u8, g: u8, b: u8) -> image::DynamicImage {
    let mut img = image::RgbImage::new(64, 64);
    for p in img.pixels_mut() {
        *p = image::Rgb([r, g, b]);
    }
    image::DynamicImage::ImageRgb8(img)
}

/// 同 [`app_with_library`],额外返回 [`TestClient`] 的队列操作记录
/// (操作菜单的插播 / 追加路径断言用)。
pub(crate) fn app_with_library_probed(
    len: usize,
    sel_track: usize,
) -> color_eyre::Result<(App, QueueOpsLog)> {
    let queue_ops = Arc::new(Mutex::new(Vec::new()));
    let client = TestClient {
        queue_ops: Arc::clone(&queue_ops),
        ..TestClient::default()
    };
    let mut app = test_app_with(Arc::new(client))?;
    let pid = PlaylistId::new(SourceKind::NETEASE, "p1");
    app.state.browse.nav.opened_playlist = Some(pid.clone());
    app.state.library.playlists = vec![playlist_view(
        "p1",
        "EndSerenading",
        SourceKind::NETEASE,
        u64::try_from(len).unwrap_or(0),
    )];
    let views = entry_views(endserenading(len));
    app.state.library.tracks.insert(
        pid,
        crate::runtime::state::PlaylistTracks {
            entries: views,
            complete: true,
            next_offset: None,
        },
    );
    app.state.browse.view.switch_to(View::Library);
    while !app.state.browse.view.at_max() {
        app.state.browse.view.tick();
    }
    app.state.browse.nav.track.set_sel(sel_track);
    Ok((app, queue_ops))
}

/// 同 [`app_with_library`],但填 `len` 首程序化生成的可区分曲目——EndSerenading
/// fixture 只有 10 首,超过一屏的滚动类测试用这个。
pub(crate) fn app_with_long_library(len: usize, sel_track: usize) -> color_eyre::Result<App> {
    let mut app = app_with_library(/*len*/ 0, /*sel_track*/ 0)?;
    let pid = PlaylistId::new(SourceKind::NETEASE, "p1");
    let songs = (0..len)
        .map(|i| {
            let mut s = mineral_test::song(&format!("t{i}"));
            s.name = format!("Track {i:02}");
            s
        })
        .collect();
    let views = entry_views(songs);
    app.state.library.tracks.insert(
        pid,
        crate::runtime::state::PlaylistTracks {
            entries: views,
            complete: true,
            next_offset: None,
        },
    );
    app.state.browse.nav.track.set_sel(sel_track);
    Ok(app)
}

/// 造一个接 [`TestClient`]、不启动图片 worker 且**已稳态进入全屏**的 [`App`]:正在播《潜在表明》、
/// 缓存逐字歌词(position 62s 落在中段),queue 填 3 首。供全屏渲染快照用。
pub(crate) fn app_in_fullscreen() -> color_eyre::Result<App> {
    Ok(seed_fullscreen(test_app()?))
}

/// 同 [`app_in_fullscreen`],但接一个记录 `seek` 目标的 [`TestClient`];额外返回该记录
/// (全屏歌词 Enter 跳到焦点行的绝对 seek 路径断言用)。
pub(crate) fn app_in_fullscreen_seek_probe() -> color_eyre::Result<(App, Arc<Mutex<Vec<u64>>>)> {
    let seeks: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
    let client = TestClient {
        seeks: Arc::clone(&seeks),
        ..TestClient::default()
    };
    Ok((seed_fullscreen(test_app_with(Arc::new(client))?), seeks))
}

/// 把一个空 [`App`] 布置成稳态全屏在播态:缓存《潜在表明》逐字歌词、position 62s 落中段、
/// queue 填 3 首、fullscreen 推到满值。[`app_in_fullscreen`] 系列共用。
fn seed_fullscreen(mut app: App) -> App {
    let track = qianzai_song();
    app.state
        .library
        .lyrics
        .insert(track.id.clone(), qianzai_lyrics());
    app.state.playback.track = Some(track.clone());
    app.state.playback.position_ms = 62_000;
    app.state.player.current = Some(track);
    app.state.player.queue = endserenading(3);
    // 稳态全屏:一步推到满值(step=1000)。
    let mut fs = Toggle::new(1);
    fs.set(true);
    fs.tick();
    app.state.browse.fullscreen = fs;
    app
}

/// 造一个接 [`TestClient`]、**已稳态进入 Search 布局态**的 [`App`]:queue 填 3 首、在播首曲。
/// 供 search 布局渲染快照用(M1 面板为占位骨架)。
pub(crate) fn app_with_search() -> color_eyre::Result<App> {
    let mut app = app_with_queue(3, /*current_idx*/ 0)?;
    // 稳态 search 布局:一步推到满值(step=1000)。
    let mut s = Toggle::new(1);
    s.set(true);
    s.tick();
    app.state.channel_search.active = s;
    Ok(app)
}

/// 造一个**已稳态进入 Search 布局态**、注入单源(NETEASE)caps 的 probed [`App`]。
///
/// `searchable` 决定默认 kind / 可搜性;额外返回 `submit_task` 任务记录(提交断言用)。
pub(crate) fn app_with_channel_search_probed(
    searchable: Vec<SearchKind>,
) -> color_eyre::Result<(App, Arc<Mutex<Vec<TaskKind>>>)> {
    let submitted = Arc::new(Mutex::new(Vec::new()));
    let client = TestClient {
        submitted: Arc::clone(&submitted),
        ..TestClient::default()
    };
    let mut app = test_app_with(Arc::new(client))?;
    app.state.caps.insert(
        SourceKind::NETEASE,
        ChannelCaps::builder()
            .searchable(searchable)
            .playlist_edit(false)
            .artist_sections(mineral_channel_core::ArtistSections::new(vec![
                mineral_channel_core::ArtistSectionKind::TopSongs,
                mineral_channel_core::ArtistSectionKind::Albums,
            ]))
            .build(),
    );
    // 真路径入会(挑默认源 + 建会话),再把 morph 推到稳态(step=1000 一步到位)。
    app.state.channel_search.enter(&app.state.caps);
    let mut active = Toggle::new(1);
    active.set(true);
    active.tick();
    app.state.channel_search.active = active;
    Ok((app, submitted))
}

/// 同 [`app_with_channel_search_probed`],但返回 `queue_ops`——供"detail 起播"类测试断言
/// atomic `play_queue` 携带 queue + exact target。
pub(crate) fn app_with_channel_search_qprobed(
    searchable: Vec<SearchKind>,
) -> color_eyre::Result<(App, QueueOpsLog)> {
    let queue_ops = Arc::new(Mutex::new(Vec::new()));
    let client = TestClient {
        queue_ops: Arc::clone(&queue_ops),
        ..TestClient::default()
    };
    let mut app = test_app_with(Arc::new(client))?;
    app.state.caps.insert(
        SourceKind::NETEASE,
        ChannelCaps::builder()
            .searchable(searchable)
            .playlist_edit(false)
            .artist_sections(mineral_channel_core::ArtistSections::new(vec![
                mineral_channel_core::ArtistSectionKind::TopSongs,
                mineral_channel_core::ArtistSectionKind::Albums,
            ]))
            .build(),
    );
    app.state.channel_search.enter(&app.state.caps);
    let mut active = Toggle::new(1);
    active.set(true);
    active.tick();
    app.state.channel_search.active = active;
    Ok((app, queue_ops))
}
