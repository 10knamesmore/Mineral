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

    /// 脚本 UI 旋钮覆盖表(opaque:只存 + 转发 + 握手重放,不解释 key)。
    pub(crate) ui_overrides: crate::ui_override::UiOverrides,

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
    ///   - `config`: daemon 配置切片(音质 / gapless 窗口 / 各间隔 / 下载目录)。
    ///   - `notify`: 事件通知双路出口(event hub + 脚本线程)。
    pub(crate) fn spawn(
        audio: AudioHandle,
        scheduler: Scheduler,
        channels: Vec<Arc<dyn MusicChannel>>,
        persist: ServerStore,
        media_cache: MediaCache,
        config: &crate::config::ServerConfig,
        notify: crate::notify::Notifier,
    ) -> Self {
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
            ui_overrides: crate::ui_override::UiOverrides::default(),
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
            crate::hook_bridge::before_play(self, song, pu);
        } else {
            mineral_log::debug!(target: "player", song_id = song.id.as_str(), source = ?song.source(), "submit SongUrl task");
            let handle = self.inner.scheduler.submit(
                TaskKind::ChannelFetch(ChannelFetchKind::SongUrl {
                    song_id: song.id.clone(),
                    quality: self.inner.playback_quality,
                }),
                Priority::User,
            );
            // player 级播放失败信号:取链失败 = 这首播不下去,提升为
            // `track_finished("error")`(脚本 / 订阅 client 可见)。
            // Cancelled(切歌砍任务)不报;迟到失败再校验一道当前曲。
            let player = self.clone();
            let failed_song = song.clone();
            tokio::spawn(async move {
                if !matches!(handle.done().await, mineral_task::TaskOutcome::Failed) {
                    return;
                }
                let still_current = player.with_state(|st| {
                    st.current_song
                        .as_ref()
                        .is_some_and(|s| s.id == failed_song.id)
                });
                if still_current {
                    player
                        .notify()
                        .track_finished(&failed_song, mineral_protocol::FinishReason::Error);
                }
            });
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
            st.queued = None;
            st.prefetch_fired_for = None;
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
            st.queued = None;
            st.prefetch_fired_for = None;
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
            st.queued = None;
            st.prefetch_fired_for = None;
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
    }
}

#[cfg(test)]
mod tests {
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
        Album, AlbumId, AlbumRef, Artist, BitRate, Lyrics, PlayUrl, Playlist, PlaylistId, Song,
        SongId, SourceKind,
    };
    use mineral_persist::ServerStore;
    use mineral_protocol::{PlayMode, PlaybackOrigin, PlayerVersions};
    use mineral_task::Scheduler;
    use mineral_test::mock::{UrlChannel, serve_once};
    use mineral_test::song;
    use parking_lot::Mutex;
    use pretty_assertions::assert_eq;

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
                .build()
        }

        async fn search_songs(&self, _query: &str, _page: Page) -> ChannelResult<SearchHits<Song>> {
            Err(Error::NotSupported)
        }

        async fn search_albums(
            &self,
            _query: &str,
            _page: Page,
        ) -> ChannelResult<SearchHits<Album>> {
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

        async fn song_urls(
            &self,
            _ids: &[SongId],
            _quality: BitRate,
        ) -> ChannelResult<Vec<PlayUrl>> {
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

        async fn on_played(
            &self,
            id: &SongId,
            completed: bool,
            listen_ms: u64,
        ) -> ChannelResult<()> {
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
            ui_overrides: crate::ui_override::UiOverrides::default(),
            state: Mutex::new(State::empty()),
            last_seen_finished_seq: AtomicU64::new(0),
            client_events: Mutex::new(Vec::new()),
            library,
            favorites_lock: tokio::sync::Mutex::new(()),
            last_session_save: Mutex::new(std::time::Instant::now()),
            playback_quality: *cfg.playback_quality(),
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
            bitrate_bps: 320_000,
            quality: BitRate::Higher,
            size: 0,
            format: mineral_model::AudioFormat::Mp3,
            bit_depth: None,
            stream_headers: Vec::new(),
            layout: mineral_model::StreamLayout::Contiguous,
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

    /// before_play 改写:hook 返回 {url, quality} → 起播用改写值、play_url 回填改写值。
    #[tokio::test]
    async fn before_play_rewrite_replaces_url_and_quality() -> color_eyre::Result<()> {
        let (core, runtime) = core_with_script(
            r#"
            mineral.hook("before_play", function(ctx)
                return { url = "https://fallback.example/b.flac", quality = "standard" }
            end)
            "#,
        )?;
        core.with_state(|st| st.current_song = Some(song("a")));
        crate::hook_bridge::before_play(&core, &song("a"), test_play_url("a")?);
        let rewritten = wait_until(|| {
            core.with_state(|st| {
                st.play_url
                    .as_ref()
                    .is_some_and(|pu| pu.url.to_string() == "https://fallback.example/b.flac")
            })
        })
        .await;
        assert!(rewritten, "play_url 应回填改写后的 URL");
        core.with_state(|st| {
            assert_eq!(
                st.play_url.as_ref().map(|pu| pu.quality),
                Some(BitRate::Standard),
                "音质应一并改写"
            );
        });
        drop(runtime);
        Ok(())
    }

    /// before_play 改写带 headers:hook 返回 {url, headers} → play_url 回填的 stream_headers 含该头。
    /// 解灰顶替进来的 B站 url 必须带 `Referer`,否则 403(header 穿透 rewrite→play)。
    #[tokio::test]
    async fn before_play_rewrite_carries_stream_headers() -> color_eyre::Result<()> {
        let (core, runtime) = core_with_script(
            r#"
            mineral.hook("before_play", function(ctx)
                return {
                    url = "https://fallback.example/b.flac",
                    headers = { {"Referer", "https://www.bilibili.com"} },
                }
            end)
            "#,
        )?;
        core.with_state(|st| st.current_song = Some(song("a")));
        crate::hook_bridge::before_play(&core, &song("a"), test_play_url("a")?);
        let carried = wait_until(|| {
            core.with_state(|st| {
                st.play_url.as_ref().is_some_and(|pu| {
                    pu.stream_headers
                        == vec![("Referer".to_owned(), "https://www.bilibili.com".to_owned())]
                })
            })
        })
        .await;
        assert!(
            carried,
            "play_url.stream_headers 应带上 hook 返回的 Referer"
        );
        drop(runtime);
        Ok(())
    }

    /// before_play 改写 URL 但未指定 layout → effective.layout 默认 `Chunked`(解灰目标常是分片流,
    /// 流式打开避免起播 stall)。回归:曾直接继承原曲 layout(网易云 `Contiguous`),改写成 B站
    /// fMP4 后被 seekable 全扫、起播卡数秒。
    #[tokio::test]
    async fn before_play_rewrite_url_defaults_chunked_layout() -> color_eyre::Result<()> {
        let (core, runtime) = core_with_script(
            r#"
            mineral.hook("before_play", function(ctx)
                return { url = "https://fallback.example/audio.m4s" }
            end)
            "#,
        )?;
        core.with_state(|st| st.current_song = Some(song("a")));
        crate::hook_bridge::before_play(&core, &song("a"), test_play_url("a")?);
        let chunked = wait_until(|| {
            core.with_state(|st| {
                st.play_url
                    .as_ref()
                    .is_some_and(|pu| pu.layout == mineral_model::StreamLayout::Chunked)
            })
        })
        .await;
        assert!(chunked, "改写 URL 后 layout 应默认 Chunked");
        drop(runtime);
        Ok(())
    }

    /// before_play 改写时脚本显式 `layout = "contiguous"` → effective.layout 用该值(压过默认 Chunked),
    /// 给「改写成直链源」的脚本一个恢复 seekable 的出口。
    #[tokio::test]
    async fn before_play_rewrite_explicit_layout_overrides_default() -> color_eyre::Result<()> {
        let (core, runtime) = core_with_script(
            r#"
            mineral.hook("before_play", function(ctx)
                return { url = "https://fallback.example/direct.mp3", layout = "contiguous" }
            end)
            "#,
        )?;
        core.with_state(|st| st.current_song = Some(song("a")));
        crate::hook_bridge::before_play(&core, &song("a"), test_play_url("a")?);
        let contiguous = wait_until(|| {
            core.with_state(|st| {
                st.play_url
                    .as_ref()
                    .is_some_and(|pu| pu.layout == mineral_model::StreamLayout::Contiguous)
            })
        })
        .await;
        assert!(contiguous, "显式 layout=contiguous 应压过默认 Chunked");
        drop(runtime);
        Ok(())
    }

    /// before_play 跳过:hook 返回 false → 不起播本曲,推进到下一首。
    #[tokio::test]
    async fn before_play_skip_advances_to_next() -> color_eyre::Result<()> {
        let (core, runtime) = core_with_script(
            r#"
            local skipped = false
            mineral.hook("before_play", function(ctx)
                -- 只跳第一次(下一首放行,避免连锁)
                if not skipped then
                    skipped = true
                    return false
                end
            end)
            "#,
        )?;
        core.with_state(|st| {
            st.queue = vec![song("a"), song("b")];
            st.queue_sel = 0;
            st.current_song = Some(song("a"));
        });
        crate::hook_bridge::before_play(&core, &song("a"), test_play_url("a")?);
        let advanced = wait_until(|| {
            core.with_state(|st| {
                st.current_song
                    .as_ref()
                    .is_some_and(|s| s.id.as_str() == "b")
            })
        })
        .await;
        assert!(advanced, "skip 后应推进到下一首");
        drop(runtime);
        Ok(())
    }

    /// before_play 放行(无 hook 命中):走原 capture 起播路径,play_url 回填原值。
    #[tokio::test]
    async fn before_play_continue_keeps_original() -> color_eyre::Result<()> {
        let (core, runtime) = core_with_script("-- 无 hook")?;
        core.with_state(|st| st.current_song = Some(song("a")));
        let original = test_play_url("a")?;
        let want = original.url.to_string();
        crate::hook_bridge::before_play(&core, &song("a"), original);
        let kept = wait_until(|| {
            core.with_state(|st| {
                st.play_url
                    .as_ref()
                    .is_some_and(|pu| pu.url.to_string() == want)
            })
        })
        .await;
        assert!(kept, "放行应回填原 URL");
        drop(runtime);
        Ok(())
    }

    /// next:Sequential 到尾返回 None,否则取下一首。
    #[test]
    fn next_sequential_stops_at_end() {
        assert!(next_in_queue(&state_with(&["a", "b", "c"], 2, PlayMode::Sequential)).is_none());
        assert_eq!(
            next_in_queue(&state_with(&["a", "b", "c"], 0, PlayMode::Sequential)),
            Some(song("b"))
        );
    }

    /// next:RepeatAll / Shuffle 在尾部环回到首,RepeatOne 原地。
    #[test]
    fn next_wraps_and_repeats_one() {
        assert_eq!(
            next_in_queue(&state_with(&["a", "b", "c"], 2, PlayMode::RepeatAll)),
            Some(song("a"))
        );
        assert_eq!(
            next_in_queue(&state_with(&["a", "b", "c"], 2, PlayMode::Shuffle)),
            Some(song("a"))
        );
        assert_eq!(
            next_in_queue(&state_with(&["a", "b", "c"], 1, PlayMode::RepeatOne)),
            Some(song("b"))
        );
    }

    /// prev:Sequential 首位返回 None,否则取上一首的下标。
    #[test]
    fn prev_sequential_stops_at_start() {
        assert!(prev_index(&state_with(&["a", "b", "c"], 0, PlayMode::Sequential)).is_none());
        assert_eq!(
            prev_index(&state_with(&["a", "b", "c"], 2, PlayMode::Sequential)),
            Some(1) // b
        );
    }

    /// prev:RepeatAll / Shuffle 在首部环回到尾,RepeatOne 原地(均以下标计)。
    #[test]
    fn prev_wraps_and_repeats_one() {
        assert_eq!(
            prev_index(&state_with(&["a", "b", "c"], 0, PlayMode::RepeatAll)),
            Some(2) // c
        );
        assert_eq!(
            prev_index(&state_with(&["a", "b", "c"], 0, PlayMode::Shuffle)),
            Some(2) // c
        );
        assert_eq!(
            prev_index(&state_with(&["a", "b", "c"], 1, PlayMode::RepeatOne)),
            Some(1) // b
        );
    }

    /// 回归:队列含交替重复曲时,顺序推进必须按下标单向前进、走到队尾后停,
    /// **不得**在两个重复副本之间来回吸附成无限循环(历史 bug:落地按歌曲身份
    /// first-match 定位,重复曲把 queue_sel 拽回首个副本)。
    #[test]
    fn advance_next_walks_past_duplicates_without_looping() {
        // gk 在下标 1/3、400 在下标 2/5;从正在播的 400(下标 2)起逐首推进。
        let mut st = state_with(
            &["intro", "gk", "400", "gk", "fish", "400", "outro"],
            2,
            PlayMode::Sequential,
        );
        let mut visited = Vec::new();
        while let Some(_song) = advance_next(&mut st) {
            visited.push(st.queue_sel);
        }
        // 2→3→4→5→6 后到队尾停;每步严格 +1,绝不回退到 1 或 2。
        assert_eq!(visited, vec![3, 4, 5, 6]);
        assert_eq!(st.queue_sel, 6, "推进到队尾后 queue_sel 停在末位");
    }

    /// 回归:advance_prev 同样按下标后退,重复曲不把 queue_sel 吸附到首个副本。
    #[test]
    fn advance_prev_steps_back_by_index_with_duplicates() {
        let mut st = state_with(&["a", "b", "a", "b"], 3, PlayMode::Sequential); // 第二个 b
        assert_eq!(
            advance_prev(&mut st).as_ref().map(|s| s.id.as_str()),
            Some("a")
        );
        assert_eq!(st.queue_sel, 2, "应退到下标 2(第二个 a),而非首个 a@0");
        assert_eq!(
            advance_prev(&mut st).as_ref().map(|s| s.id.as_str()),
            Some("b")
        );
        assert_eq!(st.queue_sel, 1);
    }

    /// 空队列时 next / prev 都返回 None。
    #[test]
    fn empty_queue_has_no_neighbors() {
        assert!(next_in_queue(&State::empty()).is_none());
        assert!(prev_index(&State::empty()).is_none());
    }

    /// queue_sel 越界被 clamp 到末位:Sequential next=None、prev=倒数第二首。
    #[test]
    fn out_of_bounds_sel_is_clamped() {
        let st = state_with(&["a", "b"], 5, PlayMode::Sequential);
        assert!(next_in_queue(&st).is_none());
        assert_eq!(prev_index(&st), Some(0)); // a
    }

    /// enter_shuffle:内容集合不变 + 当前歌置顶 + queue_sel=0 + original 存原序。
    #[test]
    fn enter_shuffle_keeps_all_and_pins_current() {
        let mut st = state_with(&["a", "b", "c", "d"], 2, PlayMode::Sequential); // current=c
        enter_shuffle(&mut st);
        assert_eq!(st.queue.first().map(|s| s.id.as_str()), Some("c"));
        assert_eq!(st.queue_sel, 0);
        assert_eq!(ids_sorted(&st.queue), vec!["a", "b", "c", "d"]);
        assert_eq!(
            st.original_queue.as_deref().map(ids),
            Some(vec!["a", "b", "c", "d"])
        );
    }

    /// enter_shuffle:空队列 no-op,不设 original。
    #[test]
    fn enter_shuffle_empty_is_noop() {
        let mut st = State::empty();
        enter_shuffle(&mut st);
        assert!(st.queue.is_empty());
        assert!(st.original_queue.is_none());
    }

    /// exit_shuffle:从 original 还原原序,queue_sel 重定位到当前歌,清 original。
    #[test]
    fn exit_shuffle_restores_order_and_relocates_sel() {
        let mut st = state_with(&["a", "b", "c", "d"], 0, PlayMode::Shuffle);
        st.queue = vec![song("c"), song("a"), song("d"), song("b")];
        st.queue_sel = 0;
        st.current_song = Some(song("c"));
        st.original_queue = Some(vec![song("a"), song("b"), song("c"), song("d")]);
        exit_shuffle(&mut st);
        assert_eq!(ids(&st.queue), vec!["a", "b", "c", "d"]);
        assert_eq!(st.queue_sel, 2); // c 在原序的下标
        assert!(st.original_queue.is_none());
    }

    /// exit_shuffle:没有 original 时 no-op。
    #[test]
    fn exit_shuffle_without_original_is_noop() {
        let mut st = state_with(&["a", "b"], 1, PlayMode::Sequential);
        st.original_queue = None;
        exit_shuffle(&mut st);
        assert_eq!(ids(&st.queue), vec!["a", "b"]);
        assert_eq!(st.queue_sel, 1);
    }

    /// apply_play_mode:目标与当前相同时 no-op。
    #[test]
    fn apply_same_mode_is_noop() {
        let mut st = state_with(&["a", "b"], 0, PlayMode::Sequential);
        apply_play_mode(&mut st, PlayMode::Sequential);
        assert_eq!(st.play_mode, PlayMode::Sequential);
        assert!(st.original_queue.is_none());
    }

    /// enter/exit shuffle 必须推进 queue_version(漏 bump = client 永远看不到洗牌结果)。
    #[test]
    fn shuffle_boundaries_bump_queue_version() {
        let mut st = state_with(&["a", "b", "c"], 1, PlayMode::Sequential);
        let v0 = st.queue_version;
        apply_play_mode(&mut st, PlayMode::Shuffle);
        assert_eq!(st.queue_version, v0 + 1, "进 Shuffle 洗牌后应 bump");
        apply_play_mode(&mut st, PlayMode::Sequential);
        assert_eq!(st.queue_version, v0 + 2, "退 Shuffle 还原后应 bump");
    }

    /// shuffle 边界的 no-op 路径(空队列进入 / 无 original 退出)不得虚涨版本。
    #[test]
    fn noop_shuffle_paths_do_not_bump() {
        let mut empty = State::empty();
        let v0 = empty.queue_version;
        enter_shuffle(&mut empty);
        assert_eq!(empty.queue_version, v0, "空队列进 Shuffle 是 no-op");

        let mut st = state_with(&["a", "b"], 1, PlayMode::Sequential);
        let v1 = st.queue_version;
        exit_shuffle(&mut st);
        assert_eq!(st.queue_version, v1, "无 original 退 Shuffle 是 no-op");
    }

    /// 非 Shuffle 边界的模式切换(RepeatAll → RepeatOne)不动 queue,不得 bump。
    #[test]
    fn mode_change_without_queue_mutation_does_not_bump() {
        let mut st = state_with(&["a", "b"], 0, PlayMode::RepeatAll);
        let v0 = st.queue_version;
        apply_play_mode(&mut st, PlayMode::RepeatOne);
        assert_eq!(st.queue_version, v0);
    }

    /// set_queue(两种模式)必须推进 queue_version。
    #[tokio::test]
    async fn set_queue_bumps_queue_version() -> color_eyre::Result<()> {
        let core = core_with(Arc::default())?;
        let v0 = core.sync(PlayerVersions::default()).versions.queue;
        core.set_queue(
            vec![song("a"), song("b")],
            &SongId::new(SourceKind::NETEASE, "a"),
        );
        let v1 = core.sync(PlayerVersions::default()).versions.queue;
        assert_eq!(v1, v0 + 1, "顺序模式 set_queue 应 bump");

        core.set_play_mode(PlayMode::Shuffle); // 进 Shuffle 本身也 bump 一次
        let v2 = core.sync(PlayerVersions::default()).versions.queue;
        core.set_queue(
            vec![song("c"), song("d")],
            &SongId::new(SourceKind::NETEASE, "c"),
        );
        let v3 = core.sync(PlayerVersions::default()).versions.queue;
        assert_eq!(v3, v2 + 1, "Shuffle 模式 set_queue 应 bump");
        Ok(())
    }

    /// play_song 清旧上下文 + 写新 current_song,必须推进 current_version。
    #[tokio::test]
    async fn play_song_bumps_current_version() -> color_eyre::Result<()> {
        let core = core_with(Arc::default())?;
        let v0 = core.sync(PlayerVersions::default()).versions.current;
        core.play_song(&song("a"));
        let v1 = core.sync(PlayerVersions::default()).versions.current;
        assert_eq!(v1, v0 + 1);
        Ok(())
    }

    /// 回归:play_song 落地时,若 `queue_sel` 已精确指向本曲(顺序推进入口预置好),
    /// 不得再按身份 first-match 回溯——否则重复曲会把下标拽回首个副本。
    #[tokio::test]
    async fn play_song_keeps_preset_queue_sel_on_duplicate() -> color_eyre::Result<()> {
        let core = core_with(Arc::default())?;
        core.with_state(|st| {
            st.queue = vec![song("a"), song("b"), song("a"), song("b")];
            st.queue_sel = 2; // 第二个 a
            st.current_song = Some(song("a"));
        });
        core.play_song(&song("a"));
        core.with_state(|st| {
            assert_eq!(st.queue_sel, 2, "已预置的精确下标须保留,不能吸附到 a@0");
        });
        Ok(())
    }

    /// play_song 的身份定位仍在:queue_sel 未指向目标曲时,按 first-match 重新定位。
    #[tokio::test]
    async fn play_song_locates_when_not_preset() -> color_eyre::Result<()> {
        let core = core_with(Arc::default())?;
        core.with_state(|st| {
            st.queue = vec![song("a"), song("b")];
            st.queue_sel = 0;
            st.current_song = Some(song("a"));
        });
        core.play_song(&song("b"));
        core.with_state(|st| assert_eq!(st.queue_sel, 1, "点播未在位的曲应重新定位"));
        Ok(())
    }

    /// LyricsReady 命中当前歌写入歌词 → bump;不命中丢弃 → 不 bump。
    #[tokio::test]
    async fn lyrics_ready_bumps_only_on_store() -> color_eyre::Result<()> {
        let core = core_with(Arc::default())?;
        core.with_state(|st| st.current_song = Some(song("a")));
        let v0 = core.sync(PlayerVersions::default()).versions.current;

        core.handle_lyrics_ready(&SongId::new(SourceKind::NETEASE, "x"), Lyrics::default());
        let v1 = core.sync(PlayerVersions::default()).versions.current;
        assert_eq!(v1, v0, "非当前歌的歌词被丢弃,不应 bump");

        core.handle_lyrics_ready(&SongId::new(SourceKind::NETEASE, "a"), Lyrics::default());
        let v2 = core.sync(PlayerVersions::default()).versions.current;
        assert_eq!(v2, v0 + 1, "命中当前歌写入歌词应 bump");
        Ok(())
    }

    /// PlayUrlReady 命中当前歌写 play_url → bump;不命中任何路由 → 不 bump。
    #[tokio::test]
    async fn play_url_ready_bumps_only_on_current() -> color_eyre::Result<()> {
        let core = core_with(Arc::default())?;
        core.with_state(|st| st.current_song = Some(song("a")));
        let v0 = core.sync(PlayerVersions::default()).versions.current;

        core.handle_play_url_ready(&SongId::new(SourceKind::NETEASE, "x"), test_play_url("x")?);
        let v1 = core.sync(PlayerVersions::default()).versions.current;
        assert_eq!(v1, v0, "无人认领的 URL 被丢弃,不应 bump");

        core.handle_play_url_ready(&SongId::new(SourceKind::NETEASE, "a"), test_play_url("a")?);
        let v2 = core.sync(PlayerVersions::default()).versions.current;
        assert_eq!(v2, v0 + 1, "命中当前歌写 play_url 应 bump");
        Ok(())
    }

    /// apply_play_mode:进入 Shuffle 触发 enter(置顶 + 存 original),退回触发 exit(还原)。
    #[test]
    fn apply_enter_then_exit_shuffle() {
        let mut st = state_with(&["a", "b", "c"], 1, PlayMode::Sequential); // current=b
        apply_play_mode(&mut st, PlayMode::Shuffle);
        assert_eq!(st.play_mode, PlayMode::Shuffle);
        assert!(st.original_queue.is_some());
        assert_eq!(st.queue.first().map(|s| s.id.as_str()), Some("b"));

        apply_play_mode(&mut st, PlayMode::Sequential);
        assert!(st.original_queue.is_none());
        assert_eq!(ids(&st.queue), vec!["a", "b", "c"]);
    }

    /// apply_play_mode:两个非 Shuffle 模式间切换不动队列、不设 original。
    #[test]
    fn apply_between_non_shuffle_keeps_queue() {
        let mut st = state_with(&["a", "b", "c"], 1, PlayMode::Sequential);
        apply_play_mode(&mut st, PlayMode::RepeatAll);
        assert_eq!(st.play_mode, PlayMode::RepeatAll);
        assert_eq!(ids(&st.queue), vec!["a", "b", "c"]);
        assert!(st.original_queue.is_none());

        apply_play_mode(&mut st, PlayMode::RepeatOne);
        assert_eq!(ids(&st.queue), vec!["a", "b", "c"]);
        assert!(st.original_queue.is_none());
    }

    /// next_song(手动跳过):对刚播完的旧歌打 `(old_id, false, position_ms)` 点。
    #[tokio::test]
    async fn next_song_records_skip_for_old_song() -> color_eyre::Result<()> {
        let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let core = core_with(calls.clone())?;
        {
            let mut st = core.inner.state.lock();
            st.queue = vec![song("a"), song("b")];
            st.queue_sel = 0;
            st.current_song = Some(song("a"));
            st.play_mode = PlayMode::Sequential;
        }
        core.next_song();
        drain_spawned().await;

        let recorded = calls.lock().clone();
        assert_eq!(recorded.len(), 1, "应只对旧歌打一次跳过点");
        let (id, completed, _listen) = recorded
            .first()
            .cloned()
            .unwrap_or_else(|| (SongId::new(SourceKind::NETEASE, "missing"), true, u64::MAX));
        assert_eq!(id, song("a").id);
        assert!(!completed, "手动跳过应记 completed=false");
        Ok(())
    }

    /// next_song:队尾(Sequential)无下一首时不切歌,也不打点。
    #[tokio::test]
    async fn next_song_at_end_records_nothing() -> color_eyre::Result<()> {
        let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let core = core_with(calls.clone())?;
        {
            let mut st = core.inner.state.lock();
            st.queue = vec![song("a"), song("b")];
            st.queue_sel = 1;
            st.current_song = Some(song("b"));
            st.play_mode = PlayMode::Sequential;
        }
        core.next_song();
        drain_spawned().await;

        assert!(calls.lock().is_empty(), "队尾无下一首,不应打点");
        Ok(())
    }

    /// prev_or_restart:进度 ≤ 阈值真正切到上一首 → 打 `(old_id, false, _)` 点。
    #[tokio::test]
    async fn prev_below_threshold_records_skip() -> color_eyre::Result<()> {
        let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let core = core_with(calls.clone())?;
        {
            let mut st = core.inner.state.lock();
            st.queue = vec![song("a"), song("b")];
            st.queue_sel = 1;
            st.current_song = Some(song("b"));
            st.play_mode = PlayMode::Sequential;
        }
        // ForceNull 起步 position_ms == 0,< 阈值,走「跳上一首」分支。
        core.prev_or_restart();
        drain_spawned().await;

        let recorded = calls.lock().clone();
        assert_eq!(recorded.len(), 1, "应对旧歌打一次跳过点");
        let (id, completed, _listen) = recorded
            .first()
            .cloned()
            .unwrap_or_else(|| (SongId::new(SourceKind::NETEASE, "missing"), true, u64::MAX));
        assert_eq!(id, song("b").id);
        assert!(!completed, "上一首跳过应记 completed=false");
        Ok(())
    }

    /// play_mode_str:各档落地为稳定 Debug 名。
    #[test]
    fn play_mode_str_is_debug_name() {
        assert_eq!(PlayMode::Sequential.name(), "Sequential");
        assert_eq!(PlayMode::Shuffle.name(), "Shuffle");
        assert_eq!(PlayMode::RepeatAll.name(), "RepeatAll");
        assert_eq!(PlayMode::RepeatOne.name(), "RepeatOne");
    }

    /// volume_pct(u8 0..=100)→ f64 0.0..=1.0:80 → 0.8。
    #[tokio::test]
    async fn snapshot_session_converts_volume() -> color_eyre::Result<()> {
        let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let core = core_with(calls)?;
        core.audio().set_volume(80);
        let snap = core.snapshot_session();
        assert!((snap.volume - 0.8).abs() < 1e-9, "80% 应映射到 0.8");
        Ok(())
    }

    /// load_session 空库返回 Ok(None)。
    #[tokio::test]
    async fn load_session_empty_returns_none() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("t.db")).await?;
        let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let core = core_with_persist(calls, persist)?;
        assert!(core.load_session().await?.is_none(), "空库应读不到会话");
        Ok(())
    }

    /// 设入队列 + 当前歌 + 模式后,组装的 [`SessionSnapshot`] 落盘再 load 读回内容一致。
    ///
    /// 注:直接 `snapshot_session()` + `session().save()` 落盘(而非依赖 background
    /// fire-and-forget 的多次并发 save —— 它们写同一单例行无确定顺序),断言数据正确。
    #[tokio::test]
    async fn save_then_load_roundtrips_queue_and_current() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("t.db")).await?;
        let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let core = core_with_persist(calls, persist.clone())?;

        core.cycle_play_mode(); // Sequential → Shuffle
        let queue = vec![song("a"), song("b"), song("c")];
        core.set_queue(queue, &song("a").id);
        core.play_song(&song("a"));
        // 组装快照并同步落盘(确定性,不依赖 spawn 顺序)。
        let assembled = core.snapshot_session();
        persist.session().save(&assembled).await?;

        let snap = core
            .load_session()
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("应读回会话"))?;
        assert_eq!(snap.queue.len(), 3, "队列长度应为 3");
        assert!(snap.queue.contains(&song("a").id), "队列应含 a");
        assert_eq!(snap.current, Some(song("a").id), "当前歌应为 a");
        assert_eq!(snap.play_mode, "Shuffle", "模式应为 Shuffle");
        Ok(())
    }

    /// 启动恢复路径:落库的模式名经 `PlayMode::from_name` 解析 + `restore_play_mode`
    /// 写回——只动模式标志,不触发洗牌边界(队列空、original_queue 不被置),不回写会话。
    #[tokio::test]
    async fn restore_play_mode_sets_flag_without_shuffle_side_effects() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("t.db")).await?;
        let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let core = core_with_persist(calls, persist.clone())?;

        // 模拟上一次会话:Shuffle 模式落盘。
        core.cycle_play_mode(); // Sequential → Shuffle
        persist.session().save(&core.snapshot_session()).await?;

        // 模拟下一次启动:新 core 读回会话,解析模式名并恢复。
        let calls2 = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let fresh = core_with_persist(calls2, persist)?;
        let snap = fresh
            .load_session()
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("应读回会话"))?;
        let mode = PlayMode::from_name(&snap.play_mode)
            .ok_or_else(|| color_eyre::eyre::eyre!("落库模式名应可解析: {}", snap.play_mode))?;
        fresh.restore_play_mode(mode);

        let st = fresh.inner.state.lock();
        assert_eq!(st.play_mode, PlayMode::Shuffle, "模式标志应恢复");
        assert!(st.queue.is_empty(), "恢复不带队列");
        assert!(
            st.original_queue.is_none(),
            "restore 不该触发 enter_shuffle 的洗牌/存原序边界"
        );
        Ok(())
    }

    /// 周期落盘的空态守卫:daemon 空闲(无当前曲、空队列)时跳过,上次会话的队列
    /// 不被空快照覆盖——那是将来队列恢复要吃的数据。
    #[tokio::test]
    async fn periodic_save_skips_empty_state_preserving_last_session() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("t.db")).await?;
        // 上次会话:真实队列同步落盘。
        let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let core = core_with_persist(calls, persist.clone())?;
        core.set_queue(vec![song("a"), song("b")], &song("a").id);
        persist.session().save(&core.snapshot_session()).await?;

        // 模拟新启动:空态 core,把节流窗口拨到已过期再触发周期检查。
        let calls2 = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let fresh = core_with_persist(calls2, persist)?;
        if let Some(past) = std::time::Instant::now().checked_sub(Duration::from_secs(60)) {
            *fresh.inner.last_session_save.lock() = past;
        }
        fresh.check_session_save();
        drain_spawned().await;

        let snap = fresh
            .load_session()
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("应读回会话"))?;
        assert_eq!(snap.queue.len(), 2, "空态周期落盘应跳过,上次队列应保留");
        Ok(())
    }

    /// fire-and-forget 的 spawn_save_session 最终能让 load 读到会话(不断言精确字段值,
    /// 只确认接线打通、数据落盘)。
    #[tokio::test]
    async fn spawn_save_session_persists_something() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("t.db")).await?;
        let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let core = core_with_persist(calls, persist)?;

        core.set_queue(vec![song("a"), song("b")], &song("a").id);
        drain_spawned().await;

        assert!(core.load_session().await?.is_some(), "save 后应能读到会话");
        Ok(())
    }

    /// SongUrl 取链失败 → player 级播放失败信号:wire 推 `TrackFinished{reason: Error}`
    /// (RecordingChannel 的 `song_urls` 恒 `Err`,任务必然 `Failed`)。
    #[tokio::test(flavor = "multi_thread")]
    async fn play_song_url_failure_notifies_error() -> color_eyre::Result<()> {
        use mineral_protocol::{Event, FinishReason};
        let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let (events_tx, mut events_rx) = tokio::sync::broadcast::channel(/*capacity*/ 8);
        let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
            calls,
            url_delay: None,
            liked_ids: None,
            playlists: None,
        })];
        let core = core_with_events(
            channels,
            ServerStore::disabled(),
            /*music_dir*/ None,
            MediaCache::disabled(),
            events_tx,
            /*script*/ None,
        )?;
        let target = song("e1");
        core.play_song(&target);
        // SongUrl 任务在 worker 上跑失败 → 监视 task 报 Error;轮询等事件(带超时)。
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match events_rx.try_recv() {
                Ok(Event::TrackFinished { song_id, reason }) => {
                    assert_eq!(song_id, target.id);
                    assert_eq!(reason, FinishReason::Error);
                    return Ok(());
                }
                Ok(_other) => {}
                Err(_empty) => {
                    if std::time::Instant::now() > deadline {
                        color_eyre::eyre::bail!("超时未收到 TrackFinished(Error)");
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    }

    /// 取链失败但用户已切走(失败的不是当前曲)→ 不报 Error(防迟到误报)。
    #[tokio::test(flavor = "multi_thread")]
    async fn stale_url_failure_does_not_notify() -> color_eyre::Result<()> {
        use mineral_protocol::Event;
        let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let (events_tx, mut events_rx) = tokio::sync::broadcast::channel(/*capacity*/ 8);
        // 失败前人为延迟:保证「切走」必然发生在任务失败之前,时序确定不 flaky。
        let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
            calls,
            url_delay: Some(Duration::from_millis(200)),
            liked_ids: None,
            playlists: None,
        })];
        let core = core_with_events(
            channels,
            ServerStore::disabled(),
            /*music_dir*/ None,
            MediaCache::disabled(),
            events_tx,
            /*script*/ None,
        )?;
        core.play_song(&song("e1"));
        // 立即切走:当前曲不再是 e1,e1 的失败(或被 cancel)不该报。
        core.with_state(|st| st.current_song = Some(song("e2")));
        tokio::time::sleep(Duration::from_millis(500)).await;
        while let Ok(event) = events_rx.try_recv() {
            assert!(
                !matches!(event, Event::TrackFinished { ref song_id, .. } if song_id == &song("e1").id),
                "已切走的失败不该报 TrackFinished,实得 {event:?}"
            );
        }
        Ok(())
    }

    /// play_song(手动切歌)应清掉过期的 gapless 预排(`queued`),防止跨切歌泄漏预排状态。
    #[tokio::test]
    async fn play_song_clears_stale_queued() -> color_eyre::Result<()> {
        let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let core = core_with(calls)?;
        {
            let mut st = core.inner.state.lock();
            st.queue = vec![song("a"), song("b")];
            st.queue_sel = 0;
            st.current_song = Some(song("a"));
            st.queued = Some(crate::gapless::Queued {
                song: song("b"),
                play_url: None,
                origin: PlaybackOrigin::Remote,
                capturing: None,
            });
        }
        core.play_song(&song("a"));
        assert!(
            core.inner.state.lock().queued.is_none(),
            "手动切歌应清掉过期预排"
        );
        Ok(())
    }

    /// play_song 无本地副本(media_cache disabled + music_dir None)→ 走远端,
    /// snapshot.play_origin == Remote(验证 play_song → State → snapshot 接线)。
    #[tokio::test]
    async fn play_song_without_local_marks_remote() -> color_eyre::Result<()> {
        let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let core = core_with(calls)?;
        core.play_song(&song("a"));
        assert_eq!(
            core.sync(PlayerVersions::default()).play_origin,
            Some(PlaybackOrigin::Remote),
            "无本地副本应标记为远端"
        );
        Ok(())
    }

    /// 一首带专辑的测试歌曲(库路径取 album/title)。
    fn song_with_album(id: &str, name: &str, album: &str) -> Song {
        Song::builder()
            .id(SongId::new(SourceKind::NETEASE, id))
            .name(name.to_owned())
            .album(Some(AlbumRef {
                id: AlbumId::new(SourceKind::NETEASE, "0"),
                name: album.to_owned(),
            }))
            .duration_ms(1000)
            .build()
    }

    /// 端到端:**真下载**一首(走进程内 HTTP server)→ **再播放** → 应解析到刚下载的文件
    /// (`origin=Download` / `quality=Lossless`,零网络、不进缓存)。
    ///
    /// 这是「下载的歌就该从下载库播」这条业务规则的端到端守卫:跨 download → resolve →
    /// State → snapshot 全链路。若下载又顺手填了缓存,play_song 会命中缓存副本(`origin=Cache`)
    /// → 此测试变红。
    // multi_thread:走真实 TCP I/O(serve_once + reqwest)且起 audio engine,二者在单线程
    // runtime 下都脆(协作调度 / engine 需多线程),重负载时 flaky;给独立 worker 线程。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn downloads_then_plays_from_download() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("t.db")).await?;
        let media_cache =
            MediaCache::open(&persist, dir.path().join("cache"), 1_000_000_000).await?;
        let music_dir = dir.path().join("music");
        let s = song_with_album("1", "捕风", "野泳");

        // 1. 真下载到 music_dir(走进程内 HTTP server)。
        let url = serve_once(b"FAKEFLACDATA".to_vec()).await?;
        let dl_channel = UrlChannel { url };
        let http = reqwest::Client::new();
        let progress = Arc::new(Mutex::new(DownloadProgress::default()));
        download_song(
            &dl_channel,
            &crate::download::DownloadEnv {
                http: &http,
                music_dir: &music_dir,
                hooks: &crate::hook_bridge::HookGate::disabled(),
            },
            &s,
            BitRate::Lossless,
            &progress,
            /*speed_tick*/ Duration::from_millis(150),
        )
        .await?;

        // 2. 用同一 music_dir + media_cache 起 core,播放。
        let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
            calls,
            url_delay: None,
            liked_ids: None,
            playlists: None,
        })];
        let core = core_with_channels(channels, persist, Some(music_dir), media_cache)?;
        core.play_song(&s);

        // 3. 应解析到刚下载的 lossless。
        let sync = core.sync(PlayerVersions::default());
        assert_eq!(
            sync.play_origin,
            Some(PlaybackOrigin::Download),
            "下载的歌应从下载库播放,而非缓存 / 网络"
        );
        let pu = sync
            .current
            .ok_or_else(|| color_eyre::eyre::eyre!("known=0 应返回 current 重段"))?
            .play_url
            .ok_or_else(|| color_eyre::eyre::eyre!("本地命中应填 play_url"))?;
        assert_eq!(pu.quality, BitRate::Lossless, "命中音质应为 lossless");
        Ok(())
    }

    /// 造一个带 event hub 接收端的 core(UI 覆盖 / 属性下发断言用)。
    fn core_with_hub() -> color_eyre::Result<(
        PlayerCore,
        tokio::sync::broadcast::Receiver<mineral_protocol::Event>,
    )> {
        let (events_tx, events_rx) = tokio::sync::broadcast::channel(/*capacity*/ 16);
        let core = core_with_events(
            Vec::new(),
            ServerStore::disabled(),
            /*music_dir*/ None,
            MediaCache::disabled(),
            events_tx,
            /*script*/ None,
        )?;
        Ok((core, events_rx))
    }

    /// apply_ui_override:存表 + 转发;同值重写与撤销不存在的 key 都不发事件。
    #[tokio::test]
    async fn ui_override_stores_forwards_and_diffs() -> color_eyre::Result<()> {
        use mineral_protocol::{BusValue, Event};
        let (core, mut events_rx) = core_with_hub()?;
        core.apply_ui_override(
            "lyrics.fullscreen_line_gap".to_owned(),
            Some(BusValue::Int(2)),
        );
        assert_eq!(
            events_rx.try_recv()?,
            Event::UiOverride {
                key: "lyrics.fullscreen_line_gap".to_owned(),
                value: Some(BusValue::Int(2)),
            }
        );
        // 同值重写:不发。
        core.apply_ui_override(
            "lyrics.fullscreen_line_gap".to_owned(),
            Some(BusValue::Int(2)),
        );
        assert!(events_rx.try_recv().is_err(), "同值重写不得重复下发");
        // 撤销不存在的 key:不发。
        core.apply_ui_override("no.such".to_owned(), None);
        assert!(events_rx.try_recv().is_err(), "撤销不存在的 key 不得下发");
        // 快照只含在表的键。
        assert_eq!(
            core.ui_overrides_snapshot(),
            vec![("lyrics.fullscreen_line_gap".to_owned(), BusValue::Int(2))]
        );
        // 真撤销:发 None + 表清空。
        core.apply_ui_override("lyrics.fullscreen_line_gap".to_owned(), None);
        assert_eq!(
            events_rx.try_recv()?,
            Event::UiOverride {
                key: "lyrics.fullscreen_line_gap".to_owned(),
                value: None,
            }
        );
        assert!(core.ui_overrides_snapshot().is_empty(), "撤销后表应为空");
        Ok(())
    }

    /// terminal 属性:上报后 check_props 下发 Table,断开清除后回 None。
    #[tokio::test]
    async fn terminal_prop_follows_report_and_clear() -> color_eyre::Result<()> {
        use mineral_protocol::Event;
        let (core, mut events_rx) = core_with_hub()?;
        core.set_terminal_state(crate::props::TerminalReport {
            rows: 50,
            cols: 220,
            fullscreen: true,
            focused: true,
        });
        core.check_props();
        let terminal_of = |rx: &mut tokio::sync::broadcast::Receiver<Event>| {
            // check_props 首轮全量产出,滤出 terminal 一项。
            let mut found = None;
            while let Ok(ev) = rx.try_recv() {
                if let Event::PropertyChanged { prop, value } = ev
                    && prop == mineral_protocol::PropName::TERMINAL
                {
                    found = Some(value);
                }
            }
            found
        };
        assert_eq!(
            terminal_of(&mut events_rx),
            Some(mineral_protocol::PropValue::Table(vec![
                ("rows".to_owned(), mineral_protocol::PropValue::Int(50)),
                ("cols".to_owned(), mineral_protocol::PropValue::Int(220)),
                (
                    "fullscreen".to_owned(),
                    mineral_protocol::PropValue::Bool(true)
                ),
                (
                    "focused".to_owned(),
                    mineral_protocol::PropValue::Bool(true)
                ),
            ]))
        );
        // 值不变:下一 tick 不再下发。
        core.check_props();
        assert_eq!(terminal_of(&mut events_rx), None, "同值不得重复下发");
        // 断开清除:回 None。
        core.clear_terminal_state();
        core.check_props();
        assert_eq!(
            terminal_of(&mut events_rx),
            Some(mineral_protocol::PropValue::None),
            "断开后 terminal 属性应回 None"
        );
        Ok(())
    }

    /// 插播插到当前位置后、追加进末尾,当前位置不动;shuffle 下 original_queue 同步。
    #[tokio::test]
    async fn queue_insert_next_and_append_keep_current() -> color_eyre::Result<()> {
        let core = core_with(Arc::default())?;
        core.set_queue(
            vec![song("a"), song("b")],
            &SongId::new(SourceKind::NETEASE, "a"),
        );
        core.queue_insert_next(song("c"));
        core.queue_append(song("d"));
        {
            let st = core.inner.state.lock();
            let ids = st
                .queue
                .iter()
                .map(|s| s.id.as_str().to_owned())
                .collect::<Vec<String>>();
            assert_eq!(ids, ["a", "c", "b", "d"]);
            assert_eq!(st.queue_sel, 0);
        }
        core.set_play_mode(PlayMode::Shuffle);
        core.queue_insert_next(song("e"));
        {
            let st = core.inner.state.lock();
            let orig = st
                .original_queue
                .as_ref()
                .ok_or_else(|| color_eyre::eyre::eyre!("shuffle 后应有 original_queue"))?;
            assert!(
                orig.iter().any(|s| s.id.as_str() == "e"),
                "original_queue 应同步插入"
            );
            assert!(st.queue.iter().any(|s| s.id.as_str() == "e"));
        }
        Ok(())
    }

    /// 只支持建单/列单的写桩 channel(写收敛链路测试用)。
    struct WritableChannel;

    #[async_trait]
    impl MusicChannel for WritableChannel {
        fn source(&self) -> SourceKind {
            SourceKind::NETEASE
        }

        fn caps(&self) -> ChannelCaps {
            ChannelCaps::builder()
                .searchable(Vec::new())
                .playlist_edit(true)
                .build()
        }

        async fn search_songs(&self, _q: &str, _p: Page) -> ChannelResult<SearchHits<Song>> {
            Err(Error::NotSupported)
        }
        async fn search_albums(&self, _q: &str, _p: Page) -> ChannelResult<SearchHits<Album>> {
            Err(Error::NotSupported)
        }
        async fn search_playlists(
            &self,
            _q: &str,
            _p: Page,
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
        async fn song_urls(&self, _ids: &[SongId], _q: BitRate) -> ChannelResult<Vec<PlayUrl>> {
            Err(Error::NotSupported)
        }
        async fn lyrics(&self, _id: &SongId) -> ChannelResult<Lyrics> {
            Err(Error::NotSupported)
        }

        async fn create_playlist(&self, name: &str) -> ChannelResult<Playlist> {
            Ok(Playlist::builder()
                .id(PlaylistId::new(SourceKind::NETEASE, "created-1"))
                .name(name.to_owned())
                .build())
        }

        async fn my_playlists(&self) -> ChannelResult<Vec<Playlist>> {
            Ok(vec![
                Playlist::builder()
                    .id(PlaylistId::new(SourceKind::NETEASE, "created-1"))
                    .name(String::from("新歌单"))
                    .build(),
            ])
        }
    }

    /// 写成功 → PlaylistWriteDone 转发给 client,且自动触发 MyPlaylists 重拉
    /// (缓存收敛走读管线,不直接改数据)。
    #[tokio::test]
    async fn playlist_write_done_forwards_and_triggers_refetch() -> color_eyre::Result<()> {
        let ch: Arc<dyn MusicChannel> = Arc::new(WritableChannel);
        let core = core_with_channels(
            vec![ch],
            ServerStore::disabled(),
            /*music_dir*/ None,
            MediaCache::disabled(),
        )?;
        let h = core.inner.scheduler.submit(
            mineral_task::TaskKind::PlaylistWrite(mineral_task::PlaylistWriteOp::Create {
                source: SourceKind::NETEASE,
                name: String::from("新歌单"),
            }),
            mineral_task::Priority::User,
        );
        assert_eq!(h.done().await, mineral_task::TaskOutcome::Ok);
        core.consume_events_once();
        let evs = core.drain_client_events();
        assert!(
            evs.iter().any(|e| matches!(
                e,
                mineral_task::TaskEvent::PlaylistWriteDone { error: None, .. }
            )),
            "写完结事件应转发给 client,got {evs:?}"
        );

        // 收敛重拉(MyPlaylists)由 consume 时提交,异步执行;逐源列表进聚合态,
        // client 收到的是出口变换后的合并快照。轮询等它落地。
        let mut found = false;
        for _ in 0..100 {
            tokio::time::sleep(Duration::from_millis(5)).await;
            core.consume_events_once();
            if core.drain_client_events().iter().any(|e| {
                matches!(
                    e,
                    mineral_task::TaskEvent::LibrarySnapshot { playlists }
                        if playlists.iter().any(|p| p.id.value() == "created-1")
                )
            }) {
                found = true;
                break;
            }
        }
        assert!(found, "写成功后应触发 MyPlaylists 重拉并推合并快照");
        Ok(())
    }

    /// 极简歌单(netease 源)。
    fn named_playlist(id: &str, name: &str) -> Playlist {
        Playlist::builder()
            .id(PlaylistId::new(SourceKind::NETEASE, id))
            .name(name.to_owned())
            .build()
    }

    /// 轮询 consume + drain 直到收到一条 `LibrarySnapshot`,返回其载荷。
    async fn wait_snapshot(core: &PlayerCore) -> color_eyre::Result<Vec<Playlist>> {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            core.consume_events_once();
            for ev in core.drain_client_events() {
                if let mineral_task::TaskEvent::LibrarySnapshot { playlists } = ev {
                    return Ok(playlists);
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        color_eyre::eyre::bail!("超时未收到 LibrarySnapshot")
    }

    /// 组装带 curate registry 函数的 core(模拟 config 管线摘取结果):
    /// per-source 函数按源名入表、跨源函数入独立键,channel 返回给定歌单。
    fn core_with_curate(
        per_source: &[(&str, &str)],
        merged: Option<&str>,
        playlists: Vec<Playlist>,
    ) -> color_eyre::Result<(PlayerCore, mineral_script::ScriptRuntime)> {
        use mineral_script::{ScriptHost, ScriptRuntime, ScriptSender, install_api};
        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let (push_tx, _push_rx) = tokio::sync::mpsc::unbounded_channel();
        let host = ScriptHost::new(cmd_tx, push_tx);
        let lua = mineral_script::mlua::Lua::new();
        install_api(&lua, &host)?;
        let fns = lua.create_table()?;
        for (source, src) in per_source {
            fns.set(
                *source,
                lua.load(*src).eval::<mineral_script::mlua::Function>()?,
            )?;
        }
        lua.set_named_registry_value(mineral_config::CURATE_PLAYLISTS_SOURCE_FNS, fns)?;
        if let Some(src) = merged {
            lua.set_named_registry_value(
                mineral_config::CURATE_PLAYLISTS_MERGED_FN,
                lua.load(src).eval::<mineral_script::mlua::Function>()?,
            )?;
        }
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
                playlists: Some(playlists),
            })],
            ServerStore::disabled(),
            /*music_dir*/ None,
            MediaCache::disabled(),
            tokio::sync::broadcast::channel(/*capacity*/ 8).0,
            Some(sender),
        )?;
        Ok((core, runtime))
    }

    /// 无脚本:各源列表进聚合态,client 收 identity 透传的合并快照。
    #[tokio::test(flavor = "multi_thread")]
    async fn library_snapshot_identity_without_script() -> color_eyre::Result<()> {
        let core = core_with_channels(
            vec![Arc::new(RecordingChannel {
                calls: Arc::default(),
                url_delay: None,
                liked_ids: None,
                playlists: Some(vec![
                    named_playlist("p1", "日常"),
                    named_playlist("p2", "稍后再看"),
                ]),
            })],
            ServerStore::disabled(),
            /*music_dir*/ None,
            MediaCache::disabled(),
        )?;
        core.submit_my_playlists(SourceKind::NETEASE);
        let snapshot = wait_snapshot(&core).await?;
        let names = snapshot
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<&str>>();
        assert_eq!(names, vec!["日常", "稍后再看"], "无 transform 原样透传");
        Ok(())
    }

    /// curate 全链:per-source 过滤 + 跨源改名都落到快照。
    #[tokio::test(flavor = "multi_thread")]
    async fn library_snapshot_applies_curate_functions() -> color_eyre::Result<()> {
        let (core, _runtime) = core_with_curate(
            &[(
                "netease",
                r#"function(lists)
                    local keep = {}
                    for _, p in ipairs(lists) do
                        if p.name ~= "稍后再看" then keep[#keep + 1] = p end
                    end
                    return keep
                end"#,
            )],
            Some(
                r#"function(all)
                    for _, p in ipairs(all) do p.name = "[" .. p.name .. "]" end
                    return all
                end"#,
            ),
            vec![
                named_playlist("p1", "日常"),
                named_playlist("p2", "稍后再看"),
            ],
        )?;
        core.submit_my_playlists(SourceKind::NETEASE);
        let snapshot = wait_snapshot(&core).await?;
        let names = snapshot
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<&str>>();
        assert_eq!(
            names,
            vec!["[日常]"],
            "per-source 滤掉稍后再看,跨源函数改名"
        );
        Ok(())
    }

    /// 拉取失败(NotSupported)也给结论:空贡献快照照常推,初始完备不卡死。
    #[tokio::test(flavor = "multi_thread")]
    async fn library_failure_concludes_with_empty_snapshot() -> color_eyre::Result<()> {
        // RecordingChannel playlists: None → my_playlists NotSupported → 任务 Failed。
        let core = core_with(Arc::default())?;
        core.submit_my_playlists(SourceKind::NETEASE);
        let snapshot = wait_snapshot(&core).await?;
        assert!(snapshot.is_empty(), "失败源空贡献,快照为空但必须到达");
        Ok(())
    }

    /// 脚本 `library.playlists` 在初始完备前停靠,完备时刻统一 resolve
    /// (config.lua 顶层调用是常态场景;快照与 client 同为出口变换结果)。
    #[tokio::test(flavor = "multi_thread")]
    async fn library_playlists_query_parks_until_complete() -> color_eyre::Result<()> {
        use mineral_script::{ScriptHost, ScriptSender, install_api};
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let (push_tx, push_rx) = tokio::sync::mpsc::unbounded_channel();
        let host = ScriptHost::new(cmd_tx.clone(), push_tx.clone());
        let lua = mineral_script::mlua::Lua::new();
        install_api(&lua, &host)?;
        // 顶层调用:此刻聚合态必然未完备 → daemon 侧停靠。
        lua.load(
            r#"
            mineral.library.playlists(function(ps, err)
                mineral.ui.toast("got:" .. #ps .. ":" .. ps[1].name)
            end)
            "#,
        )
        .exec()?;
        let parts = crate::script_bridge::ScriptParts::new(
            Some(lua),
            host,
            cmd_tx,
            cmd_rx,
            push_tx,
            push_rx,
        );
        let sender = ScriptSender::detached();
        let watchdog = mineral_script::WatchdogConfig::builder()
            .instruction_interval(10_000)
            .soft_wall(Duration::from_millis(200))
            .hard_wall(Duration::from_secs(1))
            .build();
        let channels: Vec<Arc<dyn MusicChannel>> = vec![Arc::new(RecordingChannel {
            calls: Arc::default(),
            url_delay: None,
            liked_ids: None,
            playlists: Some(vec![named_playlist("p1", "日常")]),
        })];
        let (runtime, pumps) = parts.spawn_runtime(watchdog, &sender, &channels);
        let _runtime = runtime.ok_or_else(|| color_eyre::eyre::eyre!("应有脚本线程"))?;
        let (hub_tx, mut hub_rx) = tokio::sync::broadcast::channel(/*capacity*/ 8);
        let core = core_with_events(
            channels,
            ServerStore::disabled(),
            /*music_dir*/ None,
            MediaCache::disabled(),
            hub_tx.clone(),
            Some(sender),
        )?;
        let _reload = pumps.start(core.clone(), hub_tx);
        core.submit_my_playlists(SourceKind::NETEASE);
        // 测试 core 不跑 background loop,手动 tick 驱动事件消化。
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            core.consume_events_once();
            match hub_rx.try_recv() {
                Ok(mineral_protocol::Event::Toast { content, .. }) => {
                    let text = content.iter().map(|s| s.text.as_str()).collect::<String>();
                    assert_eq!(text, "got:1:日常", "停靠 query 在完备时刻收到快照");
                    return Ok(());
                }
                Ok(_other) => {}
                Err(_empty) => {
                    if std::time::Instant::now() > deadline {
                        color_eyre::eyre::bail!("超时未收到脚本回调 toast");
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    }

    /// toggle_favorite 以本地 persist 为事实来源:即使该源 channel 的远端镜像
    /// (`set_loved`)返回 NotSupported(如 bilibili / 未登录),本地也必写。
    /// 回归:曾把写绕道 channel.set_loved,NotSupported 时本地一个字没写 → 按 f 假反馈。
    #[tokio::test]
    async fn toggle_favorite_persists_locally_even_when_remote_unsupported()
    -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("t.db")).await?;
        // RecordingChannel(源 NETEASE)的 set_loved 用 trait 默认 → NotSupported。
        let core = core_with_persist(Arc::default(), persist.clone())?;
        let id = SongId::new(SourceKind::NETEASE, "42");
        let scope = persist.scope(SourceKind::NETEASE);
        assert!(!scope.is_loved(&id).await?, "初始未收藏");

        let new = core.toggle_favorite(&id).await?;
        assert!(new, "toggle 返回新态 true");
        assert!(
            scope.is_loved(&id).await?,
            "远端镜像 NotSupported,本地 persist 仍必写 loved"
        );
        // toggle 后推 canonical(供装饰自愈,不只靠 client 乐观翻转)。
        let events = core.drain_client_events();
        let pushed = events
            .iter()
            .rev()
            .find_map(|e| match e {
                mineral_task::TaskEvent::LikedSongIdsFetched { source, ids }
                    if *source == SourceKind::NETEASE =>
                {
                    Some(ids)
                }
                _ => None,
            })
            .ok_or_else(|| color_eyre::eyre::eyre!("toggle 后应推 canonical favorited 集"))?;
        assert!(pushed.contains(&id), "推的 canonical 应含刚收藏的歌");

        let new2 = core.toggle_favorite(&id).await?;
        assert!(!new2, "再 toggle 回 false");
        assert!(!scope.is_loved(&id).await?, "本地 persist 已取消");
        Ok(())
    }

    /// sync_favorites 把远端红心导入本地 persist(add-only,不删本地),并向 client_events
    /// 推 canonical(persist)favorited 集。回归:导入不得删掉本地独有的收藏(本地为准)。
    #[tokio::test]
    async fn sync_favorites_imports_remote_add_only_and_emits() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("t.db")).await?;
        let local_only = SongId::new(SourceKind::NETEASE, "B");
        persist
            .scope(SourceKind::NETEASE)
            .set_loved(&local_only, /*loved*/ true)
            .await?;
        let core = core_with_persist(Arc::default(), persist.clone())?;

        let remote_only = SongId::new(SourceKind::NETEASE, "A");
        let remote: rustc_hash::FxHashSet<SongId> = [remote_only.clone()].into_iter().collect();
        let channel: Arc<dyn MusicChannel> = Arc::new(RecordingChannel {
            calls: Arc::default(),
            url_delay: None,
            liked_ids: Some(remote),
            playlists: None,
        });
        core.sync_favorites(SourceKind::NETEASE, channel).await;

        let ids = persist.scope(SourceKind::NETEASE).loved_ids().await?;
        assert!(ids.contains(&remote_only), "远端 A 应导入本地");
        assert!(ids.contains(&local_only), "本地独有 B 不被删(本地为准)");
        assert_eq!(ids.len(), 2, "persist 应为 A ∪ B");

        let events = core.drain_client_events();
        let last = events
            .iter()
            .rev()
            .find_map(|e| match e {
                mineral_task::TaskEvent::LikedSongIdsFetched { source, ids }
                    if *source == SourceKind::NETEASE =>
                {
                    Some(ids)
                }
                _ => None,
            })
            .ok_or_else(|| color_eyre::eyre::eyre!("应向 client 推 canonical favorited 集"))?;
        assert!(
            last.contains(&remote_only) && last.contains(&local_only),
            "推给 client 的应是 persist canonical 集(A ∪ B)"
        );
        Ok(())
    }
}
