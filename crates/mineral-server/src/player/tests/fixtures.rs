//! 不启动后台播放循环的测试组装器，按测试需要注入存储、事件、脚本和播放后端。
//!
//! 配置取自应用默认树，音频引擎使用空输出；测试自行推进任务收割与状态检查。

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use mineral_audio::{AudioHandle, AudioMode};
use mineral_channel_core::MusicChannel;
use mineral_model::{SongId, SourceKind};
use mineral_persist::ServerStore;
use mineral_playback::PlaybackRegistry;
use mineral_task::Scheduler;
use parking_lot::Mutex;

use super::backends::{RecordingChannel, test_playback_registry};
use crate::media_cache::MediaCache;
use crate::player::{Inner, PlayerCore};
use crate::state::State;

/// 造一个不 spawn 后台 loop 的 [`PlayerCore`],注入记录型 channel。
///
/// # Params:
///   - `calls`: 共享的 on_played 调用记录。
///
/// # Return:
///   组装好的 [`PlayerCore`]。
pub(super) fn core_with(
    calls: Arc<Mutex<Vec<(SongId, bool, u64)>>>,
) -> color_eyre::Result<PlayerCore> {
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
pub(super) fn core_with_persist(
    calls: Arc<Mutex<Vec<(SongId, bool, u64)>>>,
    persist: ServerStore,
) -> color_eyre::Result<PlayerCore> {
    let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
        calls,
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
///   - `channels`: 注入的音乐源(下载测试传 [`mineral_test::mock::UrlChannel`])。
///   - `persist`: 持久化句柄。
///   - `music_dir`: 下载导出根目录(`None` = 下载不可用)。
///   - `media_cache`: 注入的音频缓存(`disabled` 或真实)。
///
/// # Return:
///   组装好的 [`PlayerCore`]。
pub(super) fn core_with_channels(
    channels: Vec<Arc<dyn MusicChannel>>,
    persist: ServerStore,
    music_dir: Option<&Path>,
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

/// Builds a core with an explicit permanent download root.
pub(super) fn core_with_channels_music_dir(
    channels: Vec<Arc<dyn MusicChannel>>,
    persist: ServerStore,
    media_cache: MediaCache,
    music_dir: &Path,
) -> color_eyre::Result<PlayerCore> {
    let playback = test_playback_registry(
        &channels,
        Duration::ZERO,
        /*fail*/ false,
        /*direct*/ true,
    )?;
    core_with_events_stats_playback(
        channels,
        playback,
        persist,
        Some(music_dir),
        media_cache,
        tokio::sync::broadcast::channel(/*capacity*/ 8).0,
        /*script*/ None,
        crate::StatsRecorder::disabled(),
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
pub(super) fn core_with_script(
    script: &str,
) -> color_eyre::Result<(PlayerCore, mineral_script::ScriptRuntime)> {
    core_with_script_stats(script, crate::StatsRecorder::disabled())
}

/// 同 [`core_with_script`],但注入指定埋点句柄(hook_fires 落库断言用)。
pub(super) fn core_with_script_stats(
    script: &str,
    stats: crate::StatsRecorder,
) -> color_eyre::Result<(PlayerCore, mineral_script::ScriptRuntime)> {
    core_with_script_playback(
        script,
        stats,
        Duration::ZERO,
        /*fail*/ false,
        /*direct*/ true,
    )
}

/// Builds a script-enabled core with explicit test playback behavior.
pub(super) fn core_with_script_playback(
    script: &str,
    stats: crate::StatsRecorder,
    delay: Duration,
    fail: bool,
    direct: bool,
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
    let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
        calls: Arc::default(),
        liked_ids: None,
        playlists: None,
    })];
    let playback = test_playback_registry(&channels, delay, fail, direct)?;
    let core = core_with_events_stats_playback(
        channels,
        playback,
        ServerStore::disabled(),
        /*music_dir*/ None,
        MediaCache::disabled(),
        tokio::sync::broadcast::channel(/*capacity*/ 8).0,
        Some(sender),
        stats,
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
pub(super) fn core_with_events(
    channels: Vec<Arc<dyn MusicChannel>>,
    persist: ServerStore,
    music_dir: Option<&Path>,
    media_cache: MediaCache,
    events: tokio::sync::broadcast::Sender<mineral_protocol::Event>,
    script: Option<mineral_script::ScriptSender>,
) -> color_eyre::Result<PlayerCore> {
    core_with_events_stats(
        channels,
        persist,
        music_dir,
        media_cache,
        events,
        script,
        crate::StatsRecorder::disabled(),
    )
}

/// 同 [`core_with_events`],但注入指定埋点 recorder(端到端录制测试用真 recorder)。
///
/// # Params:
///   - `stats`: 埋点 recorder 句柄。
///
/// # Return:
///   组装好的 [`PlayerCore`]。
#[allow(clippy::too_many_arguments)] // 测试组装器:各入参是独立的注入维度,无自然分组。
pub(super) fn core_with_events_stats(
    channels: Vec<Arc<dyn MusicChannel>>,
    persist: ServerStore,
    music_dir: Option<&Path>,
    media_cache: MediaCache,
    events: tokio::sync::broadcast::Sender<mineral_protocol::Event>,
    script: Option<mineral_script::ScriptSender>,
    stats: crate::StatsRecorder,
) -> color_eyre::Result<PlayerCore> {
    let playback = test_playback_registry(
        &channels,
        Duration::ZERO,
        /*fail*/ false,
        /*direct*/ true,
    )?;
    core_with_events_stats_playback(
        channels,
        playback,
        persist,
        music_dir,
        media_cache,
        events,
        script,
        stats,
    )
}

/// Test core builder with an explicit playback registry.
#[allow(clippy::too_many_arguments)] // Test fixture injection dimensions are independent.
pub(super) fn core_with_events_stats_playback(
    channels: Vec<Arc<dyn MusicChannel>>,
    playback: PlaybackRegistry,
    persist: ServerStore,
    music_dir: Option<&Path>,
    media_cache: MediaCache,
    events: tokio::sync::broadcast::Sender<mineral_protocol::Event>,
    script: Option<mineral_script::ScriptSender>,
    stats: crate::StatsRecorder,
) -> color_eyre::Result<PlayerCore> {
    // fixture 从唯一默认配置源派生 server 切片。
    let cfg = crate::config::ServerConfig::from_config(&mineral_config::Config::defaults()?);
    let scheduler = Scheduler::new(&channels, *cfg.channel_workers_per());
    let (audio, _tap) = AudioHandle::spawn(AudioMode::ForceNull, cfg.engine().clone())?;
    let library = crate::library::Library::new(
        channels
            .iter()
            .map(|ch| ch.source())
            .collect::<Vec<SourceKind>>(),
    );
    // 测试不起打标 worker(关闭态 null-object)。
    let tagging =
        crate::tagging::TaggingQueue::spawn(/*enabled*/ false, &[], None, /*workers*/ 1);
    let notify = crate::notify::Notifier::new(events, script);
    let music_dir = music_dir.map(Path::to_path_buf);
    let downloads = crate::download::DownloadManager::spawn(
        crate::download::DownloadRuntime {
            music_dir: music_dir.clone(),
            channels: channels.clone(),
            playback: playback.clone(),
            hooks: crate::hook_bridge::HookGate::new(
                notify.script_sender(),
                Duration::from_millis(*cfg.hook_timeout_ms()),
            ),
            tagging: tagging.clone(),
            notify: notify.clone(),
            stats: stats.clone(),
            speed_tick: Duration::from_millis(*cfg.daemon().download_speed_tick_ms()),
        },
        *cfg.download().quality(),
        *cfg.download().max_concurrent(),
    );
    let inner = Arc::new(Inner {
        state_changes: crate::state::StatePublisher::new().1,
        audio,
        scheduler,
        channels,
        playback,
        persist,
        media_cache: Arc::new(media_cache),
        music_dir,
        downloads,
        tagging,
        // 多数测试无脚本;hook 拦截测试经 `core_with_script` 注入。
        notify,
        stats,
        props: crate::props::PropsWatch::default(),
        ui_state: Mutex::new(crate::props::TerminalStates::default()),
        // 真实默认树:覆盖类测试要经它过落型校验,空树会把一切覆盖判坏。
        config_host: crate::config_host::ConfigHost::new(mineral_config::default_tree()?),
        state: Mutex::new(State::empty()),
        last_seen_finished_seq: AtomicU64::new(0),
        envelope_inflight: Mutex::new(rustc_hash::FxHashSet::default()),
        library,
        favorites_lock: tokio::sync::Mutex::new(()),
        last_session_save: Mutex::new(std::time::Instant::now()),
        playback_quality: *cfg.playback_quality(),
        playback_prefetch_bytes: *cfg.engine().prefetch_bytes(),
        envelope_params: cfg.envelope().clone(),
        gapless_prefetch_ms: *cfg.daemon().gapless_prefetch_ms(),
        prev_restart_threshold_ms: *cfg.daemon().prev_restart_threshold_ms(),
        player_tick_ms: *cfg.daemon().player_tick_ms(),
        session_save: Duration::from_secs(*cfg.daemon().session_save_secs()),
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
