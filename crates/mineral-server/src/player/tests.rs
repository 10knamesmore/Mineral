//! player 单元测试的共享夹具:mock channel + `core_with*` 组装器 + 造数据 helper。
//! 各主题测试文件（[`hooks`] 等）经 `use super::*` 复用。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use async_trait::async_trait;
use mineral_audio::{AudioHandle, AudioMode};
use mineral_channel_core::{
    ChannelCaps, Error, MusicChannel, Page, Result as ChannelResult, SearchHits,
};
use mineral_model::{
    Album, AlbumId, AlbumRef, Artist, BitRate, Lyrics, PlayUrl, Playlist, PlaylistId, Song, SongId,
    SourceKind,
};
use mineral_persist::ServerStore;
use mineral_protocol::{PlayMode, PlaybackOrigin, PlayerVersions};
use mineral_task::Scheduler;
use mineral_test::mock::{UrlChannel, serve_once};
use mineral_test::song;
use parking_lot::Mutex;

use super::{DownloadProgress, Inner, MediaCache, PlayerCore, apply_play_mode};
use crate::download::download_song;
use crate::queue::{
    advance_next, advance_prev, enter_shuffle, exit_shuffle, next_in_queue, prev_index,
};
use crate::state::State;

/// 记录型 mock channel:on_played 调用进 `calls`,其余方法返回 `NotSupported`。
/// `source()` 报 `NETEASE`,与 [`mineral_test::song`] 的来源对齐,确保被路由命中。
#[derive(Default)]
struct RecordingChannel {
    /// 已记录的 on_played 调用:(歌曲 id、是否完播、收听毫秒)。
    calls: Arc<Mutex<Vec<(SongId, bool, u64)>>>,

    /// `song_urls` 失败前的人为延迟(竞态敏感的测试用它撑开时序窗口)。
    url_delay: Option<Duration>,

    /// `liked_song_ids` 返回的远端红心集;`None` → NotSupported(favorite 导入测试用 `Some`)。
    liked_ids: Option<rustc_hash::FxHashSet<SongId>>,

    /// `my_playlists` 返回的歌单列表;`None` → NotSupported(库聚合测试用 `Some`)。
    playlists: Option<Vec<Playlist>>,
}

#[async_trait]
impl MusicChannel for RecordingChannel {
    fn source(&self) -> SourceKind {
        SourceKind::NETEASE
    }

    fn caps(&self) -> ChannelCaps {
        ChannelCaps::builder()
            .searchable(Vec::new())
            .playlist_edit(false)
            .artist_sections(mineral_channel_core::ArtistSections::new(vec![
                mineral_channel_core::ArtistSectionKind::TopSongs,
                mineral_channel_core::ArtistSectionKind::Albums,
            ]))
            .build()
    }

    async fn search_songs(&self, _query: &str, _page: Page) -> ChannelResult<SearchHits<Song>> {
        Err(Error::NotSupported)
    }

    async fn search_albums(&self, _query: &str, _page: Page) -> ChannelResult<SearchHits<Album>> {
        Err(Error::NotSupported)
    }

    async fn search_playlists(
        &self,
        _query: &str,
        _page: Page,
    ) -> ChannelResult<SearchHits<Playlist>> {
        Err(Error::NotSupported)
    }

    async fn songs_detail(&self, _ids: &[SongId]) -> ChannelResult<Vec<Song>> {
        Err(Error::NotSupported)
    }

    async fn album_detail(&self, _id: &AlbumId) -> ChannelResult<Album> {
        Err(Error::NotSupported)
    }

    async fn playlist_detail(&self, _id: &PlaylistId) -> ChannelResult<Playlist> {
        Err(Error::NotSupported)
    }

    async fn song_urls(&self, _ids: &[SongId], _quality: BitRate) -> ChannelResult<Vec<PlayUrl>> {
        if let Some(delay) = self.url_delay {
            tokio::time::sleep(delay).await;
        }
        Err(Error::NotSupported)
    }

    async fn lyrics(&self, _id: &SongId) -> ChannelResult<Lyrics> {
        Err(Error::NotSupported)
    }

    async fn artist_detail(&self, _id: &mineral_model::ArtistId) -> ChannelResult<Artist> {
        Err(Error::NotSupported)
    }

    async fn on_played(&self, id: &SongId, completed: bool, listen_ms: u64) -> ChannelResult<()> {
        self.calls.lock().push((id.clone(), completed, listen_ms));
        Ok(())
    }

    async fn liked_song_ids(&self) -> ChannelResult<rustc_hash::FxHashSet<SongId>> {
        self.liked_ids.clone().ok_or(Error::NotSupported)
    }

    async fn my_playlists(&self) -> ChannelResult<Vec<Playlist>> {
        self.playlists.clone().ok_or(Error::NotSupported)
    }
}

/// 造一个不 spawn 后台 loop 的 [`PlayerCore`],注入记录型 channel。
///
/// # Params:
///   - `calls`: 共享的 on_played 调用记录。
///
/// # Return:
///   组装好的 [`PlayerCore`]。
fn core_with(calls: Arc<Mutex<Vec<(SongId, bool, u64)>>>) -> color_eyre::Result<PlayerCore> {
    core_with_persist(calls, ServerStore::disabled())
}

/// 同 [`core_with`],但允许注入指定 [`ServerStore`](会话持久化测试用真库)。
///
/// # Params:
///   - `calls`: 共享的 on_played 调用记录。
///   - `persist`: 注入的持久化句柄。
///
/// # Return:
///   组装好的 [`PlayerCore`]。
fn core_with_persist(
    calls: Arc<Mutex<Vec<(SongId, bool, u64)>>>,
    persist: ServerStore,
) -> color_eyre::Result<PlayerCore> {
    let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
        calls,
        url_delay: None,
        liked_ids: None,
        playlists: None,
    })];
    core_with_channels(
        channels,
        persist,
        /*music_dir*/ None,
        MediaCache::disabled(),
    )
}

/// 用注入的 channels + download 根目录 + 真实 [`MediaCache`] 组装 [`PlayerCore`],
/// 端到端测下载 / 本地播放解析。
///
/// # Params:
///   - `channels`: 注入的音乐源(下载测试传 [`UrlChannel`])。
///   - `persist`: 持久化句柄。
///   - `music_dir`: 下载导出根目录(`None` = 下载不可用)。
///   - `media_cache`: 注入的音频缓存(`disabled` 或真实)。
///
/// # Return:
///   组装好的 [`PlayerCore`]。
fn core_with_channels(
    channels: Vec<Arc<dyn MusicChannel>>,
    persist: ServerStore,
    music_dir: Option<PathBuf>,
    media_cache: MediaCache,
) -> color_eyre::Result<PlayerCore> {
    core_with_events(
        channels,
        persist,
        music_dir,
        media_cache,
        // 测试出口:event hub 无订阅者(send 即丢)。
        tokio::sync::broadcast::channel(/*capacity*/ 8).0,
        /*script*/ None,
    )
}

/// 组装带脚本线程的 [`PlayerCore`](hook 拦截桥测试用):eval 给定脚本,
/// 投递句柄接进 Notifier。返回的 runtime 须由调用方持有(drop 即停脚本线程)。
///
/// # Params:
///   - `script`: 要 eval 的用户脚本(注册 hook 等)。
///
/// # Return:
///   `(core, runtime)`。
fn core_with_script(
    script: &str,
) -> color_eyre::Result<(PlayerCore, mineral_script::ScriptRuntime)> {
    use mineral_script::{ScriptHost, ScriptRuntime, ScriptSender, install_api};
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (push_tx, _push_rx) = tokio::sync::mpsc::unbounded_channel();
    let host = ScriptHost::new(cmd_tx, push_tx);
    let lua = mineral_script::mlua::Lua::new();
    install_api(&lua, &host)?;
    lua.load(script).exec()?;
    let sender = ScriptSender::detached();
    let watchdog = mineral_script::WatchdogConfig::builder()
        .instruction_interval(10_000)
        .soft_wall(Duration::from_millis(200))
        .hard_wall(Duration::from_secs(1))
        .build();
    let runtime = ScriptRuntime::spawn(lua, host, watchdog, &sender)?;
    let core = core_with_events(
        vec![Arc::new(RecordingChannel {
            calls: Arc::default(),
            url_delay: None,
            liked_ids: None,
            playlists: None,
        })],
        ServerStore::disabled(),
        /*music_dir*/ None,
        MediaCache::disabled(),
        tokio::sync::broadcast::channel(/*capacity*/ 8).0,
        Some(sender),
    )?;
    Ok((core, runtime))
}

/// 同 [`core_with_channels`],但允许注入 event hub 发送端(事件断言用)。
///
/// # Params:
///   - `channels`: 注入的音乐源。
///   - `persist`: 持久化句柄。
///   - `music_dir`: 下载导出根目录。
///   - `media_cache`: 注入的音频缓存。
///   - `events`: event hub 发送端(测试持接收端断言推送)。
///
/// # Return:
///   组装好的 [`PlayerCore`]。
fn core_with_events(
    channels: Vec<Arc<dyn MusicChannel>>,
    persist: ServerStore,
    music_dir: Option<PathBuf>,
    media_cache: MediaCache,
    events: tokio::sync::broadcast::Sender<mineral_protocol::Event>,
    script: Option<mineral_script::ScriptSender>,
) -> color_eyre::Result<PlayerCore> {
    // 配置切片取 defaults(= 接线前硬编码常量),测试行为与历史一致。
    let cfg = crate::config::ServerConfig::from_config(&mineral_config::Config::defaults()?);
    let scheduler = Scheduler::new(&channels, *cfg.channel_workers_per());
    let (audio, _tap) = AudioHandle::spawn(AudioMode::ForceNull, cfg.engine().clone())?;
    let library = crate::library::Library::new(
        channels
            .iter()
            .map(|ch| ch.source())
            .collect::<Vec<SourceKind>>(),
    );
    let inner = Arc::new(Inner {
        audio,
        scheduler,
        channels,
        persist,
        media_cache: Arc::new(media_cache),
        http: None,
        music_dir,
        download_progress: Arc::new(Mutex::new(DownloadProgress::default())),
        download_tx: tokio::sync::mpsc::unbounded_channel().0,
        download_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        // 多数测试无脚本;hook 拦截测试经 `core_with_script` 注入。
        notify: crate::notify::Notifier::new(events, script),
        props: crate::props::PropsWatch::default(),
        ui_state: Mutex::new(None),
        // 真实默认树:覆盖类测试要经它过落型校验,空树会把一切覆盖判坏。
        config_host: crate::config_host::ConfigHost::new(mineral_config::default_tree()?),
        state: Mutex::new(State::empty()),
        last_seen_finished_seq: AtomicU64::new(0),
        client_events: Mutex::new(Vec::new()),
        envelope_inflight: Mutex::new(rustc_hash::FxHashSet::default()),
        library,
        favorites_lock: tokio::sync::Mutex::new(()),
        last_session_save: Mutex::new(std::time::Instant::now()),
        playback_quality: *cfg.playback_quality(),
        envelope_params: cfg.envelope().clone(),
        gapless_prefetch_ms: *cfg.daemon().gapless_prefetch_ms(),
        prev_restart_threshold_ms: *cfg.daemon().prev_restart_threshold_ms(),
        player_tick_ms: *cfg.daemon().player_tick_ms(),
        session_save: Duration::from_secs(*cfg.daemon().session_save_secs()),
        download_quality: *cfg.download().quality(),
        download_speed_tick: Duration::from_millis(*cfg.daemon().download_speed_tick_ms()),
        media_report_interval_ms: *cfg.daemon().report_interval_ms(),
        media_seek_threshold_ms: *cfg.daemon().seek_threshold_ms(),
        hook_timeout: Duration::from_millis(*cfg.hook_timeout_ms()),
        spawn_max_concurrent: *cfg.spawn_max_concurrent(),
        backfill: crate::favorites::Backfill::new(
            *cfg.favorites_backfill_chunk_size(),
            *cfg.favorites_backfill_max_concurrent(),
        ),
    });
    Ok(PlayerCore { inner })
}

/// 让出执行若干次,给 fire-and-forget 的 `tokio::spawn(on_played)` 跑完。
async fn drain_spawned() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

/// 造一个含队列的 State:queue=ids、queue_sel=sel、current=queue[sel]、mode。
fn state_with(ids: &[&str], sel: usize, mode: PlayMode) -> State {
    let mut st = State::empty();
    st.queue = ids.iter().map(|&i| song(i)).collect();
    st.queue_sel = sel;
    st.current_song = st.queue.get(sel).cloned();
    st.play_mode = mode;
    st
}

/// 取队列各歌 id(原序)。
fn ids(songs: &[Song]) -> Vec<&str> {
    songs.iter().map(|s| s.id.as_str()).collect()
}

/// 造一个指向 example.com 的远端 [`PlayUrl`](版本 bump 测试用)。
fn test_play_url(id: &str) -> color_eyre::Result<PlayUrl> {
    Ok(PlayUrl {
        song_id: SongId::new(SourceKind::NETEASE, id),
        url: mineral_model::MediaUrl::remote(&format!("https://example.com/{id}.mp3"))?,
        bitrate_bps: Some(320_000),
        quality: BitRate::Higher,
        size: None,
        format: Some(mineral_model::AudioFormat::Mp3),
        bit_depth: None,
        stream_headers: Vec::new(),
        layout: mineral_model::StreamLayout::Contiguous,
        substituted: false,
    })
}

/// 取队列各歌 id 并排序(用于「内容集合不变」断言,不看顺序)。
fn ids_sorted(songs: &[Song]) -> Vec<&str> {
    let mut v = ids(songs);
    v.sort_unstable();
    v
}

/// 轮询断言:在 deadline 内反复检查谓词(hook 拦截是 spawn 的异步任务)。
async fn wait_until(mut pred: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if pred() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

mod envelope;
mod hooks;
mod library;
mod play;
mod queue;
mod session;
mod ui;
