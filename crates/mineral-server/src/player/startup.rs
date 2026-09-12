//! 注入播放服务、存储和配置，启动打标与下载任务及播放维护循环。

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::{Duration, Instant};

use mineral_audio::AudioHandle;
use mineral_model::SourceKind;
use mineral_persist::ServerStore;
use mineral_task::Scheduler;
use parking_lot::Mutex;

use super::{Inner, PlayerCore};
use crate::download;
use crate::media_cache::MediaCache;
use crate::server::SourceBackends;
use crate::state::State;

/// [`PlayerCore::spawn`] 的配置侧参数包:daemon 切片与有效配置底树是
/// **同一次加载**的两面,成对传递。
pub(crate) struct SpawnConfig<'a> {
    /// daemon 配置切片(音质 / gapless 窗口 / 各间隔 / 下载目录)。
    pub(crate) slices: &'a crate::config::ServerConfig,

    /// 有效配置底树(加载管线产物,配置宿主的初始状态)。
    pub(crate) tree: serde_json::Value,
}

/// [`PlayerCore::spawn`] 的两个 fire-and-forget 出口:事件通知 + 埋点 recorder。
/// 合成一参避免 spawn 超 clippy 参数上限。
pub(crate) struct Sinks {
    /// 事件通知双路出口(event hub + 脚本线程)。
    pub(crate) notify: crate::notify::Notifier,

    /// 埋点 recorder(热路径 gating + fire-and-forget 落库)。
    pub(crate) stats: crate::StatsRecorder,
}

/// Local persistence and media stores injected into [`PlayerCore::spawn`].
pub(crate) struct PlayerStorage {
    /// Song metadata, library state, sessions, and durable file indexes.
    pub(crate) persist: ServerStore,

    /// Evictable playback media cache.
    pub(crate) media_cache: MediaCache,

    /// Permanent download export root; `None` means downloads are unavailable.
    pub(crate) music_dir: Option<std::path::PathBuf>,
}

impl PlayerCore {
    /// 起 PlayerCore 并 spawn 长跑 task(events drain + auto-next + prefetch tick)。
    ///
    /// # Params:
    ///   - `audio`: 底层音频引擎句柄。
    ///   - `scheduler`: 任务调度器。
    ///   - `sources`: Catalog channels and playback providers.
    ///   - `storage`: Local persistence, playback cache, and permanent download root.
    ///   - `spawn_config`: 配置侧参数包(切片 + 有效配置底树)。
    ///   - `sinks`: 事件通知 + 埋点 recorder 两个 fire-and-forget 出口。
    pub(crate) fn spawn(
        audio: AudioHandle,
        scheduler: Scheduler,
        sources: SourceBackends,
        storage: PlayerStorage,
        spawn_config: SpawnConfig<'_>,
        sinks: Sinks,
    ) -> Self {
        let SpawnConfig {
            slices: config,
            tree: config_tree,
        } = spawn_config;
        let Sinks { notify, stats } = sinks;
        let PlayerStorage {
            persist,
            media_cache,
            music_dir,
        } = storage;
        let SourceBackends { channels, playback } = sources;
        // 打标队列拉封面用的共享 client；media export 由 Playback provider 独立 open。
        let tagging_http = reqwest::Client::builder().build().ok();
        if tagging_http.is_none() {
            mineral_log::warn!(target: "download", "HTTP client 构建失败,封面打标不可用");
        }
        let tagging = crate::tagging::TaggingQueue::spawn(
            *config.download().tagging(),
            &channels,
            tagging_http.as_ref(),
            *config.download().tagging_workers(),
        );
        let library = crate::library::Library::new(
            channels
                .iter()
                .map(|ch| ch.source())
                .collect::<Vec<SourceKind>>(),
        );
        let downloads = download::DownloadManager::spawn(
            download::DownloadRuntime {
                music_dir: music_dir.clone(),
                channels: channels.clone(),
                playback: playback.clone(),
                hooks: crate::hook_bridge::HookGate::new(
                    notify.script_sender(),
                    Duration::from_millis(*config.hook_timeout_ms()),
                ),
                tagging: tagging.clone(),
                notify: notify.clone(),
                stats: stats.clone(),
                speed_tick: Duration::from_millis(*config.daemon().download_speed_tick_ms()),
            },
            *config.download().quality(),
            *config.download().max_concurrent(),
        );
        let (publisher, state_changes) = crate::state::StatePublisher::new();
        let inner = Arc::new(Inner {
            audio,
            scheduler,
            channels,
            playback,
            persist,
            media_cache: Arc::new(media_cache),
            music_dir,
            downloads,
            tagging,
            notify,
            stats,
            props: crate::props::PropsWatch::default(),
            ui_state: Mutex::new(crate::props::TerminalStates::default()),
            config_host: crate::config_host::ConfigHost::new(config_tree),
            state: Mutex::new(State::empty()),
            state_changes,
            last_seen_finished_seq: AtomicU64::new(0),
            envelope_inflight: Mutex::new(rustc_hash::FxHashSet::default()),
            library,
            favorites_lock: tokio::sync::Mutex::new(()),
            last_session_save: Mutex::new(Instant::now()),
            playback_quality: *config.playback_quality(),
            playback_prefetch_bytes: *config.engine().prefetch_bytes(),
            envelope_params: config.envelope().clone(),
            gapless_prefetch_ms: *config.daemon().gapless_prefetch_ms(),
            prev_restart_threshold_ms: *config.daemon().prev_restart_threshold_ms(),
            player_tick_ms: *config.daemon().player_tick_ms(),
            session_save: Duration::from_secs(*config.daemon().session_save_secs()),
            media_report_interval_ms: *config.daemon().report_interval_ms(),
            media_seek_threshold_ms: *config.daemon().seek_threshold_ms(),
            hook_timeout: Duration::from_millis(*config.hook_timeout_ms()),
            spawn_max_concurrent: *config.spawn_max_concurrent(),
            backfill: crate::favorites::Backfill::new(
                *config.favorites_backfill_chunk_size(),
                *config.favorites_backfill_max_concurrent(),
            ),
        });
        inner.state.lock().publisher = Some(publisher);
        let me = Self { inner };
        let bg = me.clone();
        tokio::spawn(async move { bg.background_loop().await });
        me
    }
}
