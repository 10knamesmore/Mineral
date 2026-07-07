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
    DownloadProgress, DownloadTarget, PlayMode, PlaybackOrigin, PlayerSync, PlayerVersions,
};
use mineral_task::{ChannelFetchKind, Priority, Scheduler, Snapshot, TaskEvent, TaskId, TaskKind};
use parking_lot::Mutex;
use rand::seq::SliceRandom;

use crate::download::{self, Capturing};
use crate::gapless;
use crate::media_cache::MediaCache;
use crate::queue::{advance_next, advance_prev, apply_play_mode};
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

    /// 属性 diff 的上次值缓存(background_loop 每 tick 比对)。
    pub(crate) props: crate::props::PropsWatch,

    /// client 上报的终端 UI 状态(`Request::TerminalState` 写、断开清;
    /// check_props 每 tick 采样灌 `terminal` 属性)。
    pub(crate) ui_state: Mutex<Option<crate::props::TerminalReport>>,

    /// 有效配置宿主(合成底树 + session 覆盖 + 窗口标题覆盖,见 [`crate::config_host`])。
    pub(crate) config_host: crate::config_host::ConfigHost,

    /// 播放上下文(队列/当前歌/歌词/预拉状态)。
    pub(crate) state: Mutex<State>,

    /// 已转发给 client 的最新 finished seq;auto-next 监听它。
    last_seen_finished_seq: AtomicU64,

    /// PlayUrlReady/LyricsReady 之外的 events 暂存,client drain 时取走。
    pub(crate) client_events: Mutex<Vec<TaskEvent>>,

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
    ///   - `notify`: 事件通知双路出口(event hub + 脚本线程)。
    pub(crate) fn spawn(
        audio: AudioHandle,
        scheduler: Scheduler,
        channels: Vec<Arc<dyn MusicChannel>>,
        persist: ServerStore,
        media_cache: MediaCache,
        spawn_config: SpawnConfig<'_>,
        notify: crate::notify::Notifier,
    ) -> Self {
        let SpawnConfig {
            slices: config,
            tree: config_tree,
        } = spawn_config;
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
            props: crate::props::PropsWatch::default(),
            ui_state: Mutex::new(None),
            config_host: crate::config_host::ConfigHost::new(config_tree),
            state: Mutex::new(State::empty()),
            last_seen_finished_seq: AtomicU64::new(0),
            client_events: Mutex::new(Vec::new()),
            library,
            favorites_lock: tokio::sync::Mutex::new(()),
            last_session_save: Mutex::new(Instant::now()),
            playback_quality: *config.playback_quality(),
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

    /// Client 拉走 server 已 filter 的 events(无 PlayUrlReady / LyricsReady,这俩
    /// server 自己消化了)。
    pub fn drain_client_events(&self) -> Vec<TaskEvent> {
        std::mem::take(&mut *self.inner.client_events.lock())
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
    pub fn play_song(&self, song: &Song) {
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

    /// 替换 queue。等价历史 `App::set_queue`。
    pub fn set_queue(&self, new_queue: Vec<Song>, target_id: &SongId) {
        {
            let mut st = self.inner.state.lock();
            mineral_log::info!(
                target: "player",
                len = new_queue.len(),
                target_id = target_id.as_str(),
                mode = ?st.play_mode,
                "set queue"
            );
            if matches!(st.play_mode, PlayMode::Shuffle) {
                let mut shuffled = new_queue.clone();
                shuffled.shuffle(&mut rand::rng());
                if let Some(pos) = shuffled.iter().position(|s| s.id == *target_id) {
                    shuffled.swap(0, pos);
                }
                st.original_queue = Some(new_queue);
                st.queue = shuffled;
                st.queue_sel = 0;
            } else {
                let sel = new_queue
                    .iter()
                    .position(|s| s.id == *target_id)
                    .unwrap_or(0);
                st.queue = new_queue;
                st.queue_sel = sel;
                st.original_queue = None;
            }
            // 换队列后已预排的下一曲可能不再是新队列的 next:作废,让 check_prefetch 按新队列重排。
            st.invalidate_prefetch();
            st.bump_queue();
        }
        // 取消引擎里尚未 append 的待建预排(已 append 的无法摘除,会自然播完后由边界兜底)。
        self.inner.audio.clear_next();
        self.spawn_save_session();
    }

    /// 插播:插到当前曲之后,不动播放上下文与当前曲。
    pub fn queue_insert_next(&self, song: Song) {
        {
            let mut st = self.inner.state.lock();
            crate::queue::insert_next(&mut st, song);
            // 下一首变了:作废已排的 gapless 预排,让 check_prefetch 重排
            st.invalidate_prefetch();
        }
        self.inner.audio.clear_next();
        self.spawn_save_session();
    }

    /// 追加到队列末尾,不动播放上下文与当前曲。
    /// 当前曲恰在尾部时"下一首"会变,保守作废预排(与插播同样处理)。
    pub fn queue_append(&self, song: Song) {
        {
            let mut st = self.inner.state.lock();
            crate::queue::append(&mut st, song);
            st.invalidate_prefetch();
        }
        self.inner.audio.clear_next();
        self.spawn_save_session();
    }

    /// 全部已注册 channel 的能力声明(按注册顺序)。
    pub fn channel_caps(&self) -> Vec<(SourceKind, mineral_channel_core::ChannelCaps)> {
        self.inner
            .channels
            .iter()
            .map(|ch| (ch.source(), ch.caps()))
            .collect()
    }

    /// `m` 键:PlayMode cycle + 进/退 Shuffle 边界处洗牌或还原。
    pub fn cycle_play_mode(&self) {
        {
            let mut st = self.inner.state.lock();
            let new = st.play_mode.cycle();
            apply_play_mode(&mut st, new);
        }
        self.spawn_save_session();
    }

    /// 直接设目标 PlayMode(系统媒体控件按维度写 Shuffle/LoopStatus 后塌缩成的档)。
    pub fn set_play_mode(&self, mode: PlayMode) {
        {
            let mut st = self.inner.state.lock();
            apply_play_mode(&mut st, mode);
        }
        self.spawn_save_session();
    }

    /// 启动时恢复上次会话的播放模式:只写模式标志,不走 [`Self::set_play_mode`] 的
    /// 洗牌/还原边界(此刻队列为空,无可洗),也不回写会话(快照其余字段原样)。
    ///
    /// # Params:
    ///   - `mode`: 上次会话解析出的播放模式
    pub fn restore_play_mode(&self, mode: PlayMode) {
        let mut st = self.inner.state.lock();
        st.play_mode = mode;
    }

    /// `p` 键:进度 > 阈值 → seek(0);否则跳上一首。
    pub fn prev_or_restart(&self) {
        let pos = self.inner.audio.snapshot().position_ms;
        let (old, prev) = {
            let mut st = self.inner.state.lock();
            if st.current_song.is_none() {
                return;
            }
            if pos > self.inner.prev_restart_threshold_ms {
                drop(st);
                // 回开头不算切歌/跳过,不打点。
                self.inner.audio.seek(0);
                return;
            }
            // advance_prev 把 queue_sel 钉到上一首下标,play_song 据守卫保留它(见其内注释)。
            (st.current_song.clone(), advance_prev(&mut st))
        };
        if let Some(s) = prev {
            if let Some(old) = old {
                self.spawn_on_played(old.id.clone(), /*completed*/ false, pos);
                self.inner
                    .notify
                    .track_finished(&old, mineral_protocol::FinishReason::Skip);
            }
            self.play_song(&s);
        }
    }

    /// `n` 键:按 PlayMode 切下一首。
    pub fn next_song(&self) {
        let position_ms = self.inner.audio.snapshot().position_ms;
        let (old, next) = {
            let mut st = self.inner.state.lock();
            // advance_next 把 queue_sel 钉到下一首下标,play_song 据守卫保留它(见其内注释)。
            (st.current_song.clone(), advance_next(&mut st))
        };
        if let Some(s) = next {
            if let Some(old) = old {
                self.spawn_on_played(old.id.clone(), /*completed*/ false, position_ms);
                self.inner
                    .notify
                    .track_finished(&old, mineral_protocol::FinishReason::Skip);
            }
            self.play_song(&s);
        }
    }

    /// 异步上报一次播放打点(fire-and-forget,不阻塞播放)。
    ///
    /// # Params:
    ///   - `id`: 歌曲
    ///   - `completed`: 是否完整播完(false=跳过)
    ///   - `listen_ms`: 本次收听毫秒
    pub(crate) fn spawn_on_played(&self, id: SongId, completed: bool, listen_ms: u64) {
        let Some(channel) = self.channel_for(id.namespace()) else {
            return;
        };
        let channel = channel.clone();
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

#[cfg(test)]
mod tests;
