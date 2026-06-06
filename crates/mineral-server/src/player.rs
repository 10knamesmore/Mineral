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
use crate::queue::{next_in_queue, prev_in_queue};
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
    scheduler: Scheduler,

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

    /// 播放上下文(队列/当前歌/歌词/预拉状态)。
    pub(crate) state: Mutex<State>,

    /// 已转发给 client 的最新 finished seq;auto-next 监听它。
    last_seen_finished_seq: AtomicU64,

    /// PlayUrlReady/LyricsReady 之外的 events 暂存,client drain 时取走。
    client_events: Mutex<Vec<TaskEvent>>,

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
            state: Mutex::new(State::empty()),
            last_seen_finished_seq: AtomicU64::new(0),
            client_events: Mutex::new(Vec::new()),
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

    /// 推一条瞬时提示给 client 状态栏(借 client_events 通道)。
    pub(crate) fn push_notice(&self, text: String) {
        self.inner
            .client_events
            .lock()
            .push(TaskEvent::Notice { text });
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
            if let Some(idx) = st.queue.iter().position(|s| s.id == song.id) {
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
            self.inner.audio.play(MediaUrl::Local(path));
            let mut st = self.inner.state.lock();
            st.play_url = Some(pu);
            st.bump_current();
        } else if let Some(pu) = cached_url {
            mineral_log::debug!(target: "player", song_id = song.id.as_str(), "using queued url");
            download::play_capturing(self, song, &pu, self.inner.playback_quality);
            let mut st = self.inner.state.lock();
            st.play_url = Some(pu);
            st.bump_current();
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
            let st = self.inner.state.lock();
            if st.current_song.is_none() {
                return;
            }
            if pos > self.inner.prev_restart_threshold_ms {
                drop(st);
                // 回开头不算切歌/跳过,不打点。
                self.inner.audio.seek(0);
                return;
            }
            (st.current_song.clone(), prev_in_queue(&st))
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
            let st = self.inner.state.lock();
            (st.current_song.clone(), next_in_queue(&st))
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

    /// 一次 drain scheduler events,分类:PlayUrlReady/LyricsReady 内部消化、
    /// 其余 push 到 client_events buffer。
    fn consume_events_once(&self) {
        let events = self.inner.scheduler.drain_events();
        if events.is_empty() {
            return;
        }
        let mut forward = Vec::with_capacity(events.len());
        for ev in events {
            match ev {
                TaskEvent::PlayUrlReady { song_id, play_url } => {
                    self.handle_play_url_ready(&song_id, play_url);
                }
                TaskEvent::LyricsReady { song_id, lyrics } => {
                    self.handle_lyrics_ready(&song_id, lyrics);
                }
                other => forward.push(other),
            }
        }
        if !forward.is_empty() {
            self.inner.client_events.lock().extend(forward);
        }
    }

    /// PlayUrlReady 命中当前歌 → audio.play + 写 play_url;命中正在预拉的下一首 → gapless 预排;否则丢。
    fn handle_play_url_ready(&self, song_id: &SongId, play_url: PlayUrl) {
        // 先在锁内分类(三选一),放锁后再做会重新加锁的动作(play_capturing / gapless 预排)。
        enum Route {
            Current(Option<Box<Song>>),
            Prefetch,
            Drop,
        }
        let route = {
            let mut st = self.inner.state.lock();
            let want = st.current_song.as_ref().map(|t| &t.id);
            if want == Some(song_id) {
                st.play_url = Some(play_url.clone());
                st.bump_current();
                Route::Current(st.current_song.clone().map(Box::new))
            } else if st.prefetch_fired_for.as_ref() == Some(song_id) {
                Route::Prefetch
            } else {
                Route::Drop
            }
        };
        match route {
            Route::Current(song) => {
                mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "play", "play url ready");
                if let Some(song) = song {
                    download::play_capturing(self, &song, &play_url, self.inner.playback_quality);
                }
            }
            Route::Prefetch => {
                mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "prefetch", "play url ready");
                gapless::on_prefetch_url_ready(self, song_id, play_url);
            }
            Route::Drop => {
                mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "drop", "play url ready");
            }
        }
    }

    /// LyricsReady 命中当前歌 → 写入 current_lyrics + 配对 song_id;否则丢(只缓存当前歌)。
    fn handle_lyrics_ready(&self, song_id: &SongId, lyrics: mineral_model::Lyrics) {
        let mut st = self.inner.state.lock();
        let want = st.current_song.as_ref().map(|t| &t.id);
        if want == Some(song_id) {
            mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "store", "lyrics ready");
            st.current_lyrics = Some(lyrics);
            st.current_lyrics_song_id = Some(song_id.clone());
            st.bump_current();
        } else {
            // 非当前歌,无意义,丢(只缓存当前歌)。
            mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "drop", "lyrics ready");
        }
    }

    /// 重新提交 PlaylistsFetched / LikedSongIdsFetched 任务(给新 client 用)。
    pub fn refresh_initial_loads(&self) {
        for ch in &self.inner.channels {
            let source = ch.source();
            self.inner.scheduler.submit(
                TaskKind::ChannelFetch(ChannelFetchKind::MyPlaylists { source }),
                Priority::User,
            );
            self.inner.scheduler.submit(
                TaskKind::ChannelFetch(ChannelFetchKind::LikedSongIds { source }),
                Priority::Background,
            );
        }
    }
}

/// 设置 PlayMode,并在进 / 退 Shuffle 边界处洗牌或还原 queue。模式不变则 no-op。
fn apply_play_mode(st: &mut State, new: PlayMode) {
    let old = st.play_mode;
    if old == new {
        return;
    }
    mineral_log::info!(target: "player", old = ?old, new = ?new, "play mode changed");
    st.play_mode = new;
    match (old == PlayMode::Shuffle, new == PlayMode::Shuffle) {
        (false, true) => enter_shuffle(st),
        (true, false) => exit_shuffle(st),
        _ => {}
    }
}

/// 进入 shuffle:存原序到 `original_queue`,洗牌后把当前歌挪到 0 位、`queue_sel = 0`。
fn enter_shuffle(st: &mut State) {
    if st.queue.is_empty() {
        return;
    }
    let original = st.queue.clone();
    let cur_id = st.current_song.as_ref().map(|t| t.id.clone());
    st.queue.shuffle(&mut rand::rng());
    if let Some(id) = cur_id
        && let Some(pos) = st.queue.iter().position(|s| s.id == id)
    {
        st.queue.swap(0, pos);
    }
    st.queue_sel = 0;
    st.original_queue = Some(original);
    st.bump_queue();
}

/// 退出 shuffle:从 `original_queue` 还原顺序,`queue_sel` 重新定位到当前歌。
fn exit_shuffle(st: &mut State) {
    let Some(original) = st.original_queue.take() else {
        return;
    };
    let cur_id = st.current_song.as_ref().map(|t| t.id.clone());
    st.queue = original;
    st.queue_sel = cur_id
        .and_then(|id| st.queue.iter().position(|s| s.id == id))
        .unwrap_or(0);
    st.bump_queue();
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use std::time::Duration;

    use async_trait::async_trait;
    use mineral_audio::{AudioHandle, AudioMode};
    use mineral_channel_core::{Error, MusicChannel, Page, Result as ChannelResult};
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

    use super::{
        DownloadProgress, Inner, MediaCache, PlayerCore, apply_play_mode, enter_shuffle,
        exit_shuffle, next_in_queue, prev_in_queue,
    };
    use crate::download::download_song;
    use crate::queue::play_mode_str;
    use crate::state::State;

    /// 记录型 mock channel:on_played 调用进 `calls`,其余方法返回 `NotSupported`。
    /// `source()` 报 `NETEASE`,与 [`mineral_test::song`] 的来源对齐,确保被路由命中。
    #[derive(Default)]
    struct RecordingChannel {
        /// 已记录的 on_played 调用:(歌曲 id、是否完播、收听毫秒)。
        calls: Arc<Mutex<Vec<(SongId, bool, u64)>>>,

        /// `song_urls` 失败前的人为延迟(竞态敏感的测试用它撑开时序窗口)。
        url_delay: Option<Duration>,
    }

    #[async_trait]
    impl MusicChannel for RecordingChannel {
        fn source(&self) -> SourceKind {
            SourceKind::NETEASE
        }

        async fn search_songs(&self, _query: &str, _page: Page) -> ChannelResult<Vec<Song>> {
            Err(Error::NotSupported)
        }

        async fn search_albums(&self, _query: &str, _page: Page) -> ChannelResult<Vec<Album>> {
            Err(Error::NotSupported)
        }

        async fn search_playlists(
            &self,
            _query: &str,
            _page: Page,
        ) -> ChannelResult<Vec<Playlist>> {
            Err(Error::NotSupported)
        }

        async fn songs_detail(&self, _ids: &[SongId]) -> ChannelResult<Vec<Song>> {
            Err(Error::NotSupported)
        }

        async fn songs_in_album(&self, _id: &AlbumId) -> ChannelResult<Vec<Song>> {
            Err(Error::NotSupported)
        }

        async fn songs_in_playlist(&self, _id: &PlaylistId) -> ChannelResult<Vec<Song>> {
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
        )
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
    ) -> color_eyre::Result<PlayerCore> {
        // 配置切片取 defaults(= 接线前硬编码常量),测试行为与历史一致。
        let cfg = crate::config::ServerConfig::from_config(&mineral_config::Config::defaults()?);
        let scheduler = Scheduler::new(&channels, *cfg.channel_workers_per());
        let (audio, _tap) = AudioHandle::spawn(AudioMode::ForceNull, cfg.engine().clone())?;
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
            // 无脚本(脚本路由 mineral-script 的 runtime 测试与 daemon e2e 覆盖)。
            notify: crate::notify::Notifier::new(events, /*script*/ None),
            props: crate::props::PropsWatch::default(),
            state: Mutex::new(State::empty()),
            last_seen_finished_seq: AtomicU64::new(0),
            client_events: Mutex::new(Vec::new()),
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
        })
    }

    /// 取队列各歌 id 并排序(用于「内容集合不变」断言,不看顺序)。
    fn ids_sorted(songs: &[Song]) -> Vec<&str> {
        let mut v = ids(songs);
        v.sort_unstable();
        v
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

    /// prev:Sequential 首位返回 None,否则取上一首。
    #[test]
    fn prev_sequential_stops_at_start() {
        assert!(prev_in_queue(&state_with(&["a", "b", "c"], 0, PlayMode::Sequential)).is_none());
        assert_eq!(
            prev_in_queue(&state_with(&["a", "b", "c"], 2, PlayMode::Sequential)),
            Some(song("b"))
        );
    }

    /// prev:RepeatAll / Shuffle 在首部环回到尾,RepeatOne 原地。
    #[test]
    fn prev_wraps_and_repeats_one() {
        assert_eq!(
            prev_in_queue(&state_with(&["a", "b", "c"], 0, PlayMode::RepeatAll)),
            Some(song("c"))
        );
        assert_eq!(
            prev_in_queue(&state_with(&["a", "b", "c"], 0, PlayMode::Shuffle)),
            Some(song("c"))
        );
        assert_eq!(
            prev_in_queue(&state_with(&["a", "b", "c"], 1, PlayMode::RepeatOne)),
            Some(song("b"))
        );
    }

    /// 空队列时 next / prev 都返回 None。
    #[test]
    fn empty_queue_has_no_neighbors() {
        assert!(next_in_queue(&State::empty()).is_none());
        assert!(prev_in_queue(&State::empty()).is_none());
    }

    /// queue_sel 越界被 clamp 到末位:Sequential next=None、prev=倒数第二首。
    #[test]
    fn out_of_bounds_sel_is_clamped() {
        let st = state_with(&["a", "b"], 5, PlayMode::Sequential);
        assert!(next_in_queue(&st).is_none());
        assert_eq!(prev_in_queue(&st), Some(song("a")));
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
        assert_eq!(play_mode_str(PlayMode::Sequential), "Sequential");
        assert_eq!(play_mode_str(PlayMode::Shuffle), "Shuffle");
        assert_eq!(play_mode_str(PlayMode::RepeatAll), "RepeatAll");
        assert_eq!(play_mode_str(PlayMode::RepeatOne), "RepeatOne");
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
        })];
        let core = core_with_events(
            channels,
            ServerStore::disabled(),
            /*music_dir*/ None,
            MediaCache::disabled(),
            events_tx,
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
        })];
        let core = core_with_events(
            channels,
            ServerStore::disabled(),
            /*music_dir*/ None,
            MediaCache::disabled(),
            events_tx,
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
        Song {
            id: SongId::new(SourceKind::NETEASE, id),
            name: name.to_owned(),
            artists: Vec::new(),
            album: Some(AlbumRef {
                id: AlbumId::new(SourceKind::NETEASE, "0"),
                name: album.to_owned(),
            }),
            duration_ms: 1000,
            cover_url: None,
            source_url: None,
        }
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
            &http,
            &music_dir,
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
}
