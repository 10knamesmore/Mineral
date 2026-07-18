//! 服务端 PlayerCore — 集中持有「播放上下文」(current_song / queue / play_mode /
//! play_url / current_lyrics / prefetched 等),让 daemon 自治 auto-next、不依赖 client。
//!
//! 长跑后台 loop 周期做四件事:drain scheduler events(消化 PlayUrlReady / LyricsReady,
//! 其余推 client)、auto-next(监听 `track_finished_seq`)、prefetch 下一曲 URL、harvest
//! 下完的 capture。下载走独立单 worker 串行消费队列(见 [`crate::download`])。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mineral_audio::AudioHandle;
use mineral_channel_core::MusicChannel;
use mineral_model::{BitRate, MediaUrl, PlayUrl, Song, SongId, SourceKind};
use mineral_persist::ServerStore;
use mineral_protocol::{
    DownloadProgress, DownloadTarget, PlaybackOrigin, PlayerSync, PlayerVersions,
};
use mineral_task::{ChannelFetchKind, Priority, Scheduler, Snapshot, TaskId, TaskKind};
use parking_lot::Mutex;

use crate::download::{self, Capturing};
use crate::gapless;
use crate::media_cache::MediaCache;
use crate::state::State;

/// 服务端 PlayerCore。`Clone` 通过 `Arc` 廉价。
#[derive(Clone)]
pub struct PlayerCore {
    /// 共享内部状态(audio handle / scheduler / 注入 channel / 播放上下文)。
    pub(crate) inner: Arc<Inner>,
}

/// `PlayerCore` 的真实状态。
pub(crate) struct Inner {
    /// 底层音频引擎句柄。
    pub(crate) audio: AudioHandle,

    /// 任务调度器(用于提交 SongUrl / Lyrics / Playlists 等)。
    pub(crate) scheduler: Scheduler,

    /// 已注入的 channel 列表(用于按 [`SourceKind`] 路由)。
    channels: Vec<Arc<dyn MusicChannel>>,

    /// 持久化句柄(廉价 clone,Arc 内部)。
    pub(crate) persist: ServerStore,

    /// 音频本体缓存(命中直接本地播、播完入缓存);禁用环境为 null-object。
    media_cache: Arc<MediaCache>,

    /// 下载用的 HTTP client(整段 GET);构建失败为 `None`(下载不可用)。
    http: Option<reqwest::Client>,

    /// 永久下载导出根目录(`~/Music/mineral`);播放解析据此**直接探盘**命中已下载副本、
    /// 跳过网络;解析失败为 `None`(下载不可用)。
    music_dir: Option<PathBuf>,

    /// 下载进度共享态:下载任务实时写,client(TUI 弹窗 / CLI status)轮询读。
    download_progress: Arc<Mutex<DownloadProgress>>,

    /// 下载任务入队端:`download()` 把目标投进来,单 worker 串行消费(避免并发下载竞争)。
    download_tx: tokio::sync::mpsc::UnboundedSender<DownloadTarget>,

    /// 未完成的下载批数(入队 +1、批处理完 -1);0→1 开新会话、归 0 结束会话并出完成提示。
    download_pending: Arc<std::sync::atomic::AtomicUsize>,

    /// 事件通知双路出口(event hub + 脚本线程)。
    pub(crate) notify: crate::notify::Notifier,

    /// 埋点 recorder(热路径 gating + fire-and-forget 落库);无持久化时为 disabled no-op。
    pub(crate) stats: crate::StatsRecorder,

    /// 属性 diff 的上次值缓存(background_loop 每 tick 比对)。
    pub(crate) props: crate::props::PropsWatch,

    /// 各连接上报的终端 UI 状态(`Request::TerminalState` 写、断开清;
    /// check_props 每 tick 采样灌 `terminal` 属性,last-wins 裁决见
    /// [`TerminalStates`](crate::props::TerminalStates))。
    pub(crate) ui_state: Mutex<crate::props::TerminalStates>,

    /// 有效配置宿主(合成底树 + session 覆盖 + 窗口标题覆盖,见 [`crate::config_host`])。
    pub(crate) config_host: crate::config_host::ConfigHost,

    /// 播放上下文(队列/当前歌/歌词/预拉状态)。
    pub(crate) state: Mutex<State>,

    /// 已转发给 client 的最新 finished seq;auto-next 监听它。
    last_seen_finished_seq: AtomicU64,

    /// 包络离线计算的 in-flight 守卫(qualified id):开播 / 预排 / 收割多路
    /// 触发同曲时只解码一次。
    pub(crate) envelope_inflight: Mutex<rustc_hash::FxHashSet<String>>,

    /// 用户歌单库聚合态(原始数据唯一事实源 + curate 出口变换,见 [`crate::library`])。
    library: crate::library::Library,

    /// 收藏(♥)persist 读改写 + canonical 推送的串行锁:让 toggle 与 connect 期 sync_favorites
    /// 互斥,防并发交错导致陈旧远端快照复活刚取消的收藏、或乐观收藏被整源桶替换清掉。
    /// 只护 persist 读写 + 推送,**不**跨远端网络调用(镜像 / fetch 都在锁外)。
    pub(crate) favorites_lock: tokio::sync::Mutex<()>,

    /// 上次「周期 position 刷新」落盘时刻;background_loop 按 `session_save` 节流。
    pub(crate) last_session_save: Mutex<Instant>,

    /// 在线播放音质(配置 `audio.playback_quality`,独立于下载音质)。
    playback_quality: BitRate,

    /// 响度包络计算参数(配置 `audio.envelope`)。
    pub(crate) envelope_params: mineral_audio::EnvelopeParams,

    /// gapless 预排触发距曲终的剩余时间(ms,配置 `daemon.gapless_prefetch_ms`)。
    gapless_prefetch_ms: u64,

    /// `p` 键的「回开头 vs 上一首」分界(ms,配置 `daemon.prev_restart_threshold_ms`)。
    prev_restart_threshold_ms: u64,

    /// 长跑后台 task 的醒来间隔(ms,配置 `daemon.player_tick_ms`)。
    player_tick_ms: u64,

    /// 会话「位置刷新」的节流间隔(配置 `daemon.session_save_secs`)。
    pub(crate) session_save: Duration,

    /// 下载音质(配置 `download.quality`,与播放音质各自独立)。
    download_quality: BitRate,

    /// 下载测速刷新节流间隔(配置 `daemon.download_speed_tick_ms`)。
    download_speed_tick: Duration,

    /// 系统媒体服务的播放进度上报间隔(ms,配置 `daemon.report_interval_ms`)。
    media_report_interval_ms: u64,

    /// 系统媒体服务判定 seek 的位置跳变阈值(ms,配置 `daemon.seek_threshold_ms`)。
    media_seek_threshold_ms: u64,

    /// 同步拦截 hook 软超时(配置 `script.hook_timeout_ms`)。
    hook_timeout: Duration,

    /// `mineral.spawn` 并发上限(配置 `script.spawn_max_concurrent`;0 = 不限)。
    spawn_max_concurrent: usize,

    /// 聚合收藏补 meta 后台任务的状态 + 节流旋钮(单飞闸 / 待办标志 / 并发参数,见 [`crate::favorites`])。
    pub(crate) backfill: crate::favorites::Backfill,
}

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

impl PlayerCore {
    /// 起 PlayerCore 并 spawn 长跑 task(events drain + auto-next + prefetch tick)。
    ///
    /// # Params:
    ///   - `audio`: 底层音频引擎句柄。
    ///   - `scheduler`: 任务调度器。
    ///   - `channels`: 已注入的全部音乐源 handle。
    ///   - `persist`: 持久化句柄,存入 [`Inner`] 供 B-T7 起使用。
    ///   - `media_cache`: 音频本体缓存;无音频缓存环境传 [`MediaCache::disabled`]。
    ///   - `spawn_config`: 配置侧参数包(切片 + 有效配置底树)。
    ///   - `sinks`: 事件通知 + 埋点 recorder 两个 fire-and-forget 出口。
    pub(crate) fn spawn(
        audio: AudioHandle,
        scheduler: Scheduler,
        channels: Vec<Arc<dyn MusicChannel>>,
        persist: ServerStore,
        media_cache: MediaCache,
        spawn_config: SpawnConfig<'_>,
        sinks: Sinks,
    ) -> Self {
        let SpawnConfig {
            slices: config,
            tree: config_tree,
        } = spawn_config;
        let Sinks { notify, stats } = sinks;
        let (http, music_dir) = crate::download::open_env(config.download().dir().as_deref());
        let (download_tx, download_rx) = tokio::sync::mpsc::unbounded_channel();
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
            http,
            music_dir,
            download_progress: Arc::new(Mutex::new(DownloadProgress::default())),
            download_tx,
            download_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            notify,
            stats,
            props: crate::props::PropsWatch::default(),
            ui_state: Mutex::new(crate::props::TerminalStates::default()),
            config_host: crate::config_host::ConfigHost::new(config_tree),
            state: Mutex::new(State::empty()),
            last_seen_finished_seq: AtomicU64::new(0),
            envelope_inflight: Mutex::new(rustc_hash::FxHashSet::default()),
            library,
            favorites_lock: tokio::sync::Mutex::new(()),
            last_session_save: Mutex::new(Instant::now()),
            playback_quality: *config.playback_quality(),
            envelope_params: config.envelope().clone(),
            gapless_prefetch_ms: *config.daemon().gapless_prefetch_ms(),
            prev_restart_threshold_ms: *config.daemon().prev_restart_threshold_ms(),
            player_tick_ms: *config.daemon().player_tick_ms(),
            session_save: Duration::from_secs(*config.daemon().session_save_secs()),
            download_quality: *config.download().quality(),
            download_speed_tick: Duration::from_millis(*config.daemon().download_speed_tick_ms()),
            media_report_interval_ms: *config.daemon().report_interval_ms(),
            media_seek_threshold_ms: *config.daemon().seek_threshold_ms(),
            hook_timeout: Duration::from_millis(*config.hook_timeout_ms()),
            spawn_max_concurrent: *config.spawn_max_concurrent(),
            backfill: crate::favorites::Backfill::new(
                *config.favorites_backfill_chunk_size(),
                *config.favorites_backfill_max_concurrent(),
            ),
        });
        let me = Self { inner };
        let bg = me.clone();
        tokio::spawn(async move { bg.background_loop().await });
        // 下载 worker:单线串行消费队列,所有目标聚合进同一进度会话。
        let dl = me.clone();
        let pending = Arc::clone(&me.inner.download_pending);
        tokio::spawn(async move { download::worker(dl, download_rx, pending).await });
        me
    }

    /// 版本门控同步:client 报已持版本号,仅落后部分以重段返回(语义见 [`PlayerSync`])。
    /// `known = 0` 时等价旧的全量 snapshot,启动 / tick 同一条路径。
    pub fn sync(&self, known: PlayerVersions) -> PlayerSync {
        self.inner.state.lock().sync(known)
    }

    /// 内部 AudioHandle 引用 — 给 [`crate::client::ClientHandle`] 转发 pause/seek
    /// 等无业务语义的低级操作。**不暴露**给 client trait;client 只能调 trait 方法。
    pub(crate) fn audio(&self) -> &AudioHandle {
        &self.inner.audio
    }

    /// 直通:scheduler 状态。
    pub fn task_snapshot(&self) -> Snapshot {
        self.inner.scheduler.snapshot()
    }

    /// 用户歌单库聚合态句柄(管线与脚本查询在 [`crate::library`] 消费)。
    pub(crate) fn library(&self) -> &crate::library::Library {
        &self.inner.library
    }

    /// 按 [`SourceKind`] 找对应的已注入 channel handle;无匹配返回 `None`。
    ///
    /// # Params:
    ///   - `source`: 目标音乐源。
    ///
    /// # Return:
    ///   命中的 channel handle 引用,无则 `None`。
    pub(crate) fn channel_for(&self, source: SourceKind) -> Option<&Arc<dyn MusicChannel>> {
        self.inner.channels.iter().find(|ch| ch.source() == source)
    }

    /// 已注入的全部音乐源(脚本 `library.playlists` 跨源聚合用)。
    pub(crate) fn channels(&self) -> &[Arc<dyn MusicChannel>] {
        &self.inner.channels
    }

    /// 音频本体缓存句柄引用(下载 / capture 编排在 [`crate::download`] 复用)。
    pub(crate) fn media_cache(&self) -> &Arc<MediaCache> {
        &self.inner.media_cache
    }

    /// 下载用 HTTP client;构建失败为 `None`(下载不可用)。
    pub(crate) fn http(&self) -> Option<&reqwest::Client> {
        self.inner.http.as_ref()
    }

    /// 永久下载导出根目录;解析失败为 `None`(下载不可用)。
    pub(crate) fn music_dir(&self) -> Option<&std::path::Path> {
        self.inner.music_dir.as_deref()
    }

    /// 下载进度共享态句柄(下载任务实时写入)。
    pub(crate) fn progress_handle(&self) -> &Arc<Mutex<DownloadProgress>> {
        &self.inner.download_progress
    }

    /// 在线播放音质(配置 `audio.playback_quality`)。
    pub(crate) fn playback_quality(&self) -> BitRate {
        self.inner.playback_quality
    }

    /// gapless 预排触发距曲终的剩余时间(ms,配置 `daemon.gapless_prefetch_ms`)。
    pub(crate) fn gapless_prefetch_ms(&self) -> u64 {
        self.inner.gapless_prefetch_ms
    }

    /// 下载音质(配置 `download.quality`)。
    pub(crate) fn download_quality(&self) -> BitRate {
        self.inner.download_quality
    }

    /// 同步拦截 hook 软超时(配置 `script.hook_timeout_ms`)。
    pub(crate) fn hook_timeout(&self) -> Duration {
        self.inner.hook_timeout
    }

    /// `mineral.spawn` 并发上限(配置 `script.spawn_max_concurrent`;0 = 不限)。
    pub(crate) fn spawn_max_concurrent(&self) -> usize {
        self.inner.spawn_max_concurrent
    }

    /// 回填当前曲的 `play_url` 并 bump(拦截桥起播 / 改写后调)。
    ///
    /// 同值幂等:`handle_play_url_ready` 已在锁内写过原值,放行路径这里
    /// 不再重复 bump;改写路径值变了才 bump。
    ///
    /// # Params:
    ///   - `pu`: 生效的播放 URL(放行 = 原 URL,改写 = 改写后)
    pub(crate) fn set_play_url(&self, pu: PlayUrl) {
        let mut st = self.inner.state.lock();
        if st.play_url.as_ref() == Some(&pu) {
            return;
        }
        st.play_url = Some(pu);
        st.bump_current();
    }

    /// 下载测速刷新节流间隔(配置 `daemon.download_speed_tick_ms`)。
    pub(crate) fn download_speed_tick(&self) -> Duration {
        self.inner.download_speed_tick
    }

    /// 系统媒体服务的播放进度上报间隔(ms,配置 `daemon.report_interval_ms`)。
    pub(crate) fn media_report_interval_ms(&self) -> u64 {
        self.inner.media_report_interval_ms
    }

    /// 系统媒体服务判定 seek 的位置跳变阈值(ms,配置 `daemon.seek_threshold_ms`)。
    pub(crate) fn media_seek_threshold_ms(&self) -> u64 {
        self.inner.media_seek_threshold_ms
    }

    /// 当前下载进度快照(client 轮询:TUI 弹窗 / CLI status)。
    pub(crate) fn download_progress(&self) -> DownloadProgress {
        self.inner.download_progress.lock().clone()
    }

    /// 登记当前 capture 上下文(供播完 / 下完后 harvest)。
    pub(crate) fn set_capturing(&self, cap: Capturing) {
        self.inner.state.lock().capturing = Some(cap);
    }

    /// 把下载目标入队:单 worker 串行消费,聚合进同一进度会话(再点一个 → total 累加,如 2/21→2/24)。
    pub(crate) fn download(&self, target: DownloadTarget) {
        // 入队即记账:新会话(pending 0→1)重置计数;单曲已知数立刻 +1(歌单数等 worker 拉到再加)。
        let first = self.inner.download_pending.fetch_add(1, Ordering::AcqRel) == 0;
        {
            let mut p = self.inner.download_progress.lock();
            if first {
                *p = DownloadProgress {
                    active: true,
                    result_seq: p.result_seq,
                    ..DownloadProgress::default()
                };
            }
            if matches!(target, DownloadTarget::Song(_)) {
                p.total += 1;
            }
        }
        let _ = self.inner.download_tx.send(target);
    }

    /// 持久化句柄引用,供 [`crate::client::ClientHandle`] 查 love / 统计。
    ///
    /// # Return:
    ///   内部 [`ServerStore`] 句柄引用。
    pub(crate) fn persist(&self) -> &ServerStore {
        &self.inner.persist
    }

    /// 心跳用:是否已预排好下一首(gapless)。
    pub(crate) fn prefetched_ready(&self) -> bool {
        self.inner.state.lock().queued.is_some()
    }

    /// 在锁内对播放状态跑一个闭包并返回其结果(gapless 编排在 [`crate::gapless`] 复用)。
    /// **不要**在闭包里再调本方法 —— `parking_lot::Mutex` 不可重入。
    ///
    /// # Params:
    ///   - `f`: 在 `&mut State` 上执行的闭包
    pub(crate) fn with_state<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        f(&mut self.inner.state.lock())
    }

    /// gapless 边界推进读已见 finished_seq(与 snapshot 的比对)。
    pub(crate) fn last_seen_finished_seq(&self) -> u64 {
        self.inner.last_seen_finished_seq.load(Ordering::Relaxed)
    }

    /// gapless 边界推进写已见 finished_seq(消费掉一次曲终事件)。
    pub(crate) fn set_last_seen_finished_seq(&self, seq: u64) {
        self.inner
            .last_seen_finished_seq
            .store(seq, Ordering::Relaxed);
    }

    /// 底层 audio handle 的播放状态快照(gapless 编排读 finished_seq / playing / 下完标记)。
    pub(crate) fn audio_snapshot(&self) -> mineral_audio::AudioSnapshot {
        self.inner.audio.snapshot()
    }

    /// 直通:client submit 任务(playlists/tracks 类 prefetch)。
    pub fn submit_task(&self, kind: TaskKind, priority: Priority) -> TaskId {
        self.inner.scheduler.submit(kind, priority).id
    }

    /// 直通:client cancel(用于切 view 时砍 prefetch)。
    pub fn cancel_tasks_where<F>(&self, pred: F)
    where
        F: Fn(&TaskKind) -> bool + Send + Sync,
    {
        self.inner.scheduler.cancel_where(pred);
    }

    // ---- player 业务 ----

    /// Enter 一首歌。等价历史 `App::submit_play_song`。
    ///
    /// **不结算上一首**:自然 EOF / next / prev / stop 各路径已在调用前按各自语义
    /// (eof / skip / stop)结算;直接改播入口(client `PlaySong` / 脚本 `Play`)须先调
    /// [`Self::settle_interrupted`],否则被打断曲经 recorder 的自愈防线按近似值兜底。
    ///
    /// # Params:
    ///   - `song`: 要播放的歌
    ///   - `play_origin`: 起播来源(埋点 provenance:显式点播 / 自动接续 / 脚本)
    ///   - `actor`: 发起方(用户按键 / 脚本 / daemon 自治;与 origin 独立)
    pub fn play_song(
        &self,
        song: &Song,
        play_origin: mineral_stats::PlayOrigin,
        actor: mineral_stats::Actor,
    ) {
        mineral_log::info!(
            target: "player",
            song_id = song.id.as_str(),
            title = %song.name,
            "play song"
        );
        // 砍旧 SongUrl + Lyrics(切歌瞬间)
        self.inner.scheduler.cancel_where(|k| {
            matches!(
                k,
                TaskKind::ChannelFetch(
                    ChannelFetchKind::SongUrl { .. } | ChannelFetchKind::Lyrics { .. }
                )
            )
        });
        // 在 stop 前抓快照:download_complete 此刻仍对应被打断的那首。
        let prev_download_complete = self.inner.audio.snapshot().download_complete;
        self.inner.audio.stop();

        // 命中本地副本?(cache 或 download 导出,音质 >= 配置的播放音质)
        // → 直接本地播,跳过整条 SongUrl 网络路径。
        let local_hit = crate::resolve::resolve_local(
            &self.inner.media_cache,
            self.inner.music_dir.as_deref(),
            song,
            self.inner.playback_quality,
        );
        // 来源:本地命中 → cache/download(resolve 已分辨);否则(prefetch / fetch)→ 远端。
        let origin = local_hit
            .as_ref()
            .map_or(PlaybackOrigin::Remote, |&(_, _, o)| o);

        let (cached_url, interrupted, stale_queued) = {
            let mut st = self.inner.state.lock();
            st.current_song = Some(song.clone());
            // 仅当 queue_sel 尚未指向本曲时才按身份 first-match 定位(列表点歌入口)。
            // 顺序推进入口(advance_next/advance_prev)已把 queue_sel 钉到精确下标,这里
            // 必须保留——否则队列里有重复曲时,first-match 会把下标拽回首个副本,两首交替
            // 的重复曲互相指回对方、无限循环跳不出去。
            let already_positioned = st.queue.get(st.queue_sel).is_some_and(|s| s.id == song.id);
            if !already_positioned && let Some(idx) = st.queue.iter().position(|s| s.id == song.id)
            {
                st.queue_sel = idx;
            }
            st.play_url = None;
            st.play_origin = Some(origin);
            st.current_lyrics = None;
            st.current_lyrics_song_id = None;
            st.prefetch_fired_for = None;
            // 手动切歌 = 本预取窗口结束,窗口内的否决一并作废(queued 在下面按命中与否消费)。
            st.prefetch_vetoed.clear();
            st.bump_current();
            // 打断上一首未完成的 capture(残件待删)。
            let interrupted = st.capturing.take();
            // 已预排的下一曲(gapless):命中目标则复用其 URL(引擎里的预排 decoder 已被上面的
            // audio.stop() 清掉,只省一次取链);否则丢弃,其半截 capture 残件待删。
            let queued = st.queued.take();
            let (cached_url, stale_queued) = match queued {
                Some(q) if q.song.id == song.id => (q.play_url, None),
                other => (None, other),
            };
            (cached_url, interrupted, stale_queued)
        };
        // 埋点:起播语境快照(origin / actor 由调用点穿透;context 经 take_play_context
        // 消费 per-song 覆盖或继承队列级语境;format 等 play_url 快照随后经 enrich 补;
        // 时钟异常拿不出起播时刻则本次不记)。
        let play_mode = self.with_state(|st| st.play_mode);
        let context = self.take_play_context(&song.id);
        if let Some(pending) = crate::pending_from_start(
            song.clone(),
            crate::stats_play_mode(play_mode),
            song.duration_ms.and_then(|d| i64::try_from(d).ok()),
            origin,
            play_origin,
            actor,
            context,
        ) {
            self.inner.stats.play_started(pending);
        }
        if let Some(cap) = interrupted {
            // 切歌时若该曲已下完(且 harvest 轮询还没来得及处理)→ 照样入缓存;否则是 half,删残件。
            if prev_download_complete {
                download::spawn_harvest(self, cap);
            } else {
                drop(std::fs::remove_file(&cap.path));
            }
        }
        if let Some(cap) = stale_queued.and_then(|q| q.capturing) {
            // 被丢弃的预排曲:删其半截 capture 残件。
            drop(std::fs::remove_file(&cap.path));
        }
        // 对齐 finished_seq,防止 audio.stop() 极端时序下被旧 seq 误触发。
        let seq = self.inner.audio.snapshot().track_finished_seq;
        self.inner
            .last_seen_finished_seq
            .store(seq, Ordering::Relaxed);

        if let Some((path, quality, _)) = local_hit {
            mineral_log::debug!(target: "player", song_id = song.id.as_str(), action = "local_hit", quality = quality.as_str(), origin = ?origin, "本地命中,跳过网络");
            // 本地可完整读取:确保包络可用并推给 client(db 命中直推,缺失离线补算)。
            self.ensure_envelope(song.id.clone(), path.clone());
            // 本地播也填 play_url(format / bitrate 按文件内容经 lofty 读出,见 resolve),transport 才显 fmt。
            let pu = crate::resolve::local_play_url(song, &path, quality);
            self.inner
                .audio
                .play(MediaUrl::Local(path), Vec::new(), pu.layout);
            let mut st = self.inner.state.lock();
            st.play_url = Some(pu);
            st.bump_current();
        } else if let Some(pu) = cached_url {
            mineral_log::debug!(target: "player", song_id = song.id.as_str(), "using queued url");
            // 拦截桥:无脚本同步直走(play_capturing + 回填 play_url),有脚本异步裁决。
            crate::hook_bridge::before_stream(self, song, pu);
        } else {
            mineral_log::debug!(target: "player", song_id = song.id.as_str(), source = ?song.source(), "submit SongUrl task");
            // 取链失败不在这里旁听 handle:失败经 `SongUrlFailed` 事件分流到
            // `handle_song_url_failed` → unplayable 拦截口(脚本可改写补救;无脚本维持
            // track_finished("error") 原语义;Cancelled 不发事件,与旧行为一致)。
            self.inner.scheduler.submit(
                TaskKind::ChannelFetch(ChannelFetchKind::SongUrl {
                    song_id: song.id.clone(),
                    quality: self.inner.playback_quality,
                }),
                Priority::User,
            );
        }
        mineral_log::debug!(target: "player", song_id = song.id.as_str(), source = ?song.source(), "submit Lyrics task");
        self.inner.scheduler.submit(
            TaskKind::ChannelFetch(ChannelFetchKind::Lyrics {
                song_id: song.id.clone(),
            }),
            Priority::User,
        );
        self.spawn_save_session();
    }

    /// 取该曲的起播语境:优先消费 per-song 覆盖(插队散曲,取后移除),否则继承队列级
    /// 语境。所有起播路径(play_song / gapless adopt)统一走这里,防止某条路径绕过覆盖
    /// 把插队曲记成队列归属。
    ///
    /// # Params:
    ///   - `id`: 起播曲 id
    ///
    /// # Return:
    ///   该曲应记入 plays 的语境
    pub(crate) fn take_play_context(&self, id: &SongId) -> mineral_stats::QueueContext {
        self.with_state(|st| {
            st.context_overrides
                .remove(&id.qualified())
                .unwrap_or_else(|| st.queue_context.clone())
        })
    }

    /// 结算被直接改播打断的在播曲(按 skip):直接改播入口(client `PlaySong` / 脚本
    /// `Play`)在调 [`Self::play_song`] 前用。next / prev / EOF / stop 各有自己的结算,
    /// 不走这里。无在播曲时 no-op。
    ///
    /// 必须在新曲 `play_song` **之前**调用:结算取的是被打断曲的实时播放位置,且
    /// recorder 按 FIFO 先消化本结算再开新 pending。
    pub(crate) fn settle_interrupted(&self) {
        let Some(old) = self.with_state(|st| st.current_song.clone()) else {
            return;
        };
        let position_ms = self.inner.audio.snapshot().position_ms;
        self.spawn_on_played(
            old.id.clone(),
            mineral_stats::FinishReason::Skip,
            position_ms,
        );
        self.inner
            .notify
            .track_finished(&old, mineral_protocol::FinishReason::Skip);
    }

    /// 以 play_url 快照富化在播行的音频列(格式 / 码率 / 音质 / 位深 / 顶换标记)。
    /// 三个 play_url 落点(取链就绪 / gapless 轮转 / 脚本改写)统一走这里,顶换标记
    /// 才不会漏。
    ///
    /// # Params:
    ///   - `play_url`: 已生效的播放 URL 快照
    pub(crate) fn enrich_from_play_url(&self, play_url: &mineral_model::PlayUrl) {
        self.inner
            .stats
            .enrich_play_audio(mineral_stats::PlayAudioSnapshot {
                audio_format: play_url.format.clone(),
                bitrate_bps: play_url.bitrate_bps.map(i64::from),
                quality: Some(play_url.quality),
                bit_depth: play_url.bit_depth.map(i64::from),
                substituted: play_url.substituted,
            });
    }

    /// 异步上报一次播放打点(fire-and-forget,不阻塞播放)。
    ///
    /// # Params:
    ///   - `id`: 歌曲
    ///   - `reason`: 结束原因(自然播完 eof / 被切 skip;stop / error 走各自站点)
    ///   - `listen_ms`: 本次收听毫秒
    pub(crate) fn spawn_on_played(
        &self,
        id: SongId,
        reason: mineral_stats::FinishReason,
        listen_ms: u64,
    ) {
        // 埋点:结算在播行(起播被 gate 掉时 actor 无 pending、自动忽略)。
        self.inner
            .stats
            .play_ended(reason, i64::try_from(listen_ms).unwrap_or(i64::MAX));
        let Some(channel) = self.channel_for(id.namespace()) else {
            return;
        };
        let channel = channel.clone();
        // channel 契约的上报语义是完成布尔:eof 视为完整播完,其余为中断。
        let completed = matches!(reason, mineral_stats::FinishReason::Eof);
        tokio::spawn(async move {
            if let Err(e) = channel.on_played(&id, completed, listen_ms).await {
                mineral_log::warn!(target: "player", error = mineral_log::chain(&e), "on_played 打点失败");
            }
        });
    }

    // ---- 长跑后台 task ----

    /// 长跑后台 loop:每 tick 一次 events drain + harvest + auto-next + prefetch 检查。
    async fn background_loop(self) {
        let mut tick = tokio::time::interval(Duration::from_millis(self.inner.player_tick_ms));
        loop {
            tick.tick().await;
            self.consume_events_once();
            gapless::check_harvest(&self);
            gapless::check_advance(&self);
            gapless::check_prefetch(&self);
            self.check_props();
            self.check_session_save();
        }
    }

    /// 新 client 的初始数据:先即时下发歌单库缓存快照(有数据才发,消除连接
    /// 瞬间的空库假象),再重拉各源歌单 + 触发收藏同步。收藏走 server 侧 async
    /// 编排(需 persist,不进 task lane),把 canonical favorited 集推给 client。
    pub fn refresh_initial_loads(&self) {
        self.push_cached_library_snapshot();
        // 重连的 client 播放中途接入:补推当前曲的 db 包络(缺失静默,不触发计算)。
        self.replay_current_envelope();
        for ch in &self.inner.channels {
            let source = ch.source();
            self.submit_my_playlists(source);
            let this = self.clone();
            let channel = Arc::clone(ch);
            tokio::spawn(async move {
                this.sync_favorites(source, channel).await;
            });
        }
        // 立即扫一次上一会话遗留的缺 meta 收藏;本次 sync 晚到的那批由各 sync 末尾再触发、
        // pending 合并进同一 worker。
        self.spawn_meta_backfill();
    }
}

mod control;
mod transport;

#[cfg(test)]
mod tests;
