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
use mineral_persist::{Persist, SessionSnapshot};
use mineral_protocol::{DownloadProgress, DownloadTarget, PlayMode, PlayerSnapshot};
use mineral_task::{ChannelFetchKind, Priority, Scheduler, Snapshot, TaskEvent, TaskId, TaskKind};
use parking_lot::Mutex;
use rand::seq::SliceRandom;

use crate::download::{self, Capturing};
use crate::media_cache::MediaCache;
use crate::queue::{next_in_queue, play_mode_str, prev_in_queue};

/// 播放音质。后续接 config 时改读配置。
const PLAYBACK_QUALITY: BitRate = BitRate::Lossless;

/// auto-next 预拉触发距曲终的剩余时间(ms)。
const PREFETCH_LEAD_MS: u64 = 5_000;

/// `p` 键的「回开头 vs 上一首」分界(ms)。
const PREV_RESTART_THRESHOLD_MS: u64 = 3_000;

/// 长跑 task 的醒来间隔。20ms 远小于 client 30fps tick,事件最坏延迟 ~50ms。
const TICK_INTERVAL_MS: u64 = 20;

/// 会话「位置刷新」的节流间隔:background_loop 每隔这么久落盘一次 position。
/// 状态变化(切歌/换队列/改模式)有各自的即时 save,这里只为周期刷新进度。
const SESSION_SAVE_INTERVAL: Duration = Duration::from_secs(15);

/// 服务端持有的「播放上下文」内部状态。`PlayerCore` 用 `Mutex<State>` 包它。
///
/// `pub(crate)` 是为了让 [`crate::queue`] 的队列计算函数直接读 queue/queue_sel/play_mode。
pub(crate) struct State {
    /// 当前在播的歌。
    current_song: Option<Song>,

    /// 当前歌的播放 URL(从 SongUrlReady 写入)。
    play_url: Option<PlayUrl>,

    /// 当前队列(顺序模式 = 原序;shuffle 模式 = 洗过)。
    pub(crate) queue: Vec<Song>,

    /// 当前歌在 `queue` 中的下标。
    pub(crate) queue_sel: usize,

    /// shuffle 切换前的原始顺序,关 shuffle 时还原用;非 shuffle 模式下为 `None`。
    original_queue: Option<Vec<Song>>,

    /// 当前播放模式(顺序 / 单曲 / 列表循环 / shuffle)。
    pub(crate) play_mode: PlayMode,

    /// 当前歌的歌词(从 LyricsReady 写入)。
    current_lyrics: Option<mineral_model::Lyrics>,

    /// 当前 lyrics 配对的歌 id(对不上 current_song 时不返回)。
    current_lyrics_song_id: Option<SongId>,

    /// 已预拉的下一首播放 URL。
    prefetched: Option<PlayUrl>,

    /// 已对哪首歌触发过预拉(本歌只 fire 一次)。
    prefetch_fired_for: Option<SongId>,

    /// 当前正在 capture(边播边落盘)的曲;自然播完 → 入缓存,中途打断 → 删残件。
    /// 命中缓存直接本地播时为 `None`(无需 capture)。
    capturing: Option<Capturing>,
}

impl State {
    /// 空 State,所有字段取默认/空值。
    fn empty() -> Self {
        Self {
            current_song: None,
            play_url: None,
            queue: Vec::new(),
            queue_sel: 0,
            original_queue: None,
            play_mode: PlayMode::default(),
            current_lyrics: None,
            current_lyrics_song_id: None,
            prefetched: None,
            prefetch_fired_for: None,
            capturing: None,
        }
    }

    /// 从内部 State 拷出一份 [`PlayerSnapshot`] 给 client(廉价 clone)。
    fn snapshot(&self) -> PlayerSnapshot {
        PlayerSnapshot {
            current_song: self.current_song.clone(),
            play_url: self.play_url.clone(),
            queue: self.queue.clone(),
            queue_sel: self.queue_sel,
            original_queue: self.original_queue.clone(),
            play_mode: self.play_mode,
            current_lyrics: self.current_lyrics.clone(),
            current_lyrics_song_id: self.current_lyrics_song_id.clone(),
        }
    }
}

/// 服务端 PlayerCore。`Clone` 通过 `Arc` 廉价。
#[derive(Clone)]
pub struct PlayerCore {
    /// 共享内部状态(audio handle / scheduler / 注入 channel / 播放上下文)。
    inner: Arc<Inner>,
}

/// `PlayerCore` 的真实状态。
struct Inner {
    /// 底层音频引擎句柄。
    audio: AudioHandle,

    /// 任务调度器(用于提交 SongUrl / Lyrics / Playlists 等)。
    scheduler: Scheduler,

    /// 已注入的 channel 列表(用于按 [`SourceKind`] 路由)。
    channels: Vec<Arc<dyn MusicChannel>>,

    /// 持久化句柄(廉价 clone,Arc 内部)。
    persist: Persist,

    /// 音频本体缓存(命中直接本地播、播完入缓存);禁用环境为 null-object。
    media_cache: Arc<MediaCache>,

    /// 下载用的 HTTP client(整段 GET);构建失败为 `None`(下载不可用)。
    http: Option<reqwest::Client>,

    /// 永久下载导出根目录(`~/Music/mineral`);解析失败为 `None`(下载不可用)。
    music_dir: Option<PathBuf>,

    /// 下载进度共享态:下载任务实时写,client(TUI 弹窗 / CLI status)轮询读。
    download_progress: Arc<Mutex<DownloadProgress>>,

    /// 下载任务入队端:`download()` 把目标投进来,单 worker 串行消费(避免并发下载竞争)。
    download_tx: tokio::sync::mpsc::UnboundedSender<DownloadTarget>,

    /// 未完成的下载批数(入队 +1、批处理完 -1);0→1 开新会话、归 0 结束会话并出完成提示。
    download_pending: Arc<std::sync::atomic::AtomicUsize>,

    /// 播放上下文(队列/当前歌/歌词/预拉状态)。
    state: Mutex<State>,

    /// 已转发给 client 的最新 finished seq;auto-next 监听它。
    last_seen_finished_seq: AtomicU64,

    /// PlayUrlReady/LyricsReady 之外的 events 暂存,client drain 时取走。
    client_events: Mutex<Vec<TaskEvent>>,

    /// 上次「周期 position 刷新」落盘时刻;background_loop 按 [`SESSION_SAVE_INTERVAL`] 节流。
    last_session_save: Mutex<Instant>,
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
    pub(crate) fn spawn(
        audio: AudioHandle,
        scheduler: Scheduler,
        channels: Vec<Arc<dyn MusicChannel>>,
        persist: Persist,
        media_cache: MediaCache,
    ) -> Self {
        let (http, music_dir) = crate::download::open_env();
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
            state: Mutex::new(State::empty()),
            last_seen_finished_seq: AtomicU64::new(0),
            client_events: Mutex::new(Vec::new()),
            last_session_save: Mutex::new(Instant::now()),
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

    /// PlayerSnapshot — client 重连时拉一份灌进 UI 镜像。
    pub fn snapshot(&self) -> PlayerSnapshot {
        self.inner.state.lock().snapshot()
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
    ///   内部 [`Persist`] 句柄引用。
    pub(crate) fn persist(&self) -> &Persist {
        &self.inner.persist
    }

    /// 心跳用:是否已预拉好下一首的播放 URL。
    pub(crate) fn prefetched_ready(&self) -> bool {
        self.inner.state.lock().prefetched.is_some()
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

        // 命中本地音频缓存?(离线/重听 → 直接本地播,跳过整条 SongUrl 网络路径)
        let cache_hit = self.inner.media_cache.get(&song.id, PLAYBACK_QUALITY);

        let (cached_url, interrupted) = {
            let mut st = self.inner.state.lock();
            st.current_song = Some(song.clone());
            if let Some(idx) = st.queue.iter().position(|s| s.id == song.id) {
                st.queue_sel = idx;
            }
            st.play_url = None;
            st.current_lyrics = None;
            st.current_lyrics_song_id = None;
            st.prefetch_fired_for = None;
            // 打断上一首未完成的 capture(残件待删)。
            let interrupted = st.capturing.take();
            // 命中 prefetch 的 take 出来,留 None。
            let pre = match st.prefetched.take() {
                Some(pu) if pu.song_id == song.id => Some(pu),
                _ => None,
            };
            (pre, interrupted)
        };
        if let Some(cap) = interrupted {
            // 切歌时若该曲已下完(且 harvest 轮询还没来得及处理)→ 照样入缓存;否则是 half,删残件。
            if prev_download_complete {
                download::spawn_harvest(self, cap);
            } else {
                drop(std::fs::remove_file(&cap.path));
            }
        }
        // 对齐 finished_seq,防止 audio.stop() 极端时序下被旧 seq 误触发。
        let seq = self.inner.audio.snapshot().track_finished_seq;
        self.inner
            .last_seen_finished_seq
            .store(seq, Ordering::Relaxed);

        if let Some(path) = cache_hit {
            mineral_log::debug!(target: "player", song_id = song.id.as_str(), action = "cache_hit", "media cache hit");
            self.inner.audio.play(MediaUrl::Local(path));
        } else if let Some(pu) = cached_url {
            mineral_log::debug!(target: "player", song_id = song.id.as_str(), "using prefetched url");
            download::play_capturing(self, song, &pu, PLAYBACK_QUALITY);
            self.inner.state.lock().play_url = Some(pu);
        } else {
            mineral_log::debug!(target: "player", song_id = song.id.as_str(), source = ?song.source(), "submit SongUrl task");
            self.inner.scheduler.submit(
                TaskKind::ChannelFetch(ChannelFetchKind::SongUrl {
                    song_id: song.id.clone(),
                    quality: PLAYBACK_QUALITY,
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
        }
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

    /// `p` 键:进度 > 阈值 → seek(0);否则跳上一首。
    pub fn prev_or_restart(&self) {
        let pos = self.inner.audio.snapshot().position_ms;
        let (old_id, prev) = {
            let st = self.inner.state.lock();
            if st.current_song.is_none() {
                return;
            }
            if pos > PREV_RESTART_THRESHOLD_MS {
                drop(st);
                // 回开头不算切歌/跳过,不打点。
                self.inner.audio.seek(0);
                return;
            }
            (
                st.current_song.as_ref().map(|s| s.id.clone()),
                prev_in_queue(&st),
            )
        };
        if let Some(s) = prev {
            if let Some(old) = old_id {
                self.spawn_on_played(old, /*completed*/ false, pos);
            }
            self.play_song(&s);
        }
    }

    /// `n` 键:按 PlayMode 切下一首。
    pub fn next_song(&self) {
        let position_ms = self.inner.audio.snapshot().position_ms;
        let (old_id, next) = {
            let st = self.inner.state.lock();
            (
                st.current_song.as_ref().map(|s| s.id.clone()),
                next_in_queue(&st),
            )
        };
        if let Some(s) = next {
            if let Some(old) = old_id {
                self.spawn_on_played(old, /*completed*/ false, position_ms);
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
    fn spawn_on_played(&self, id: SongId, completed: bool, listen_ms: u64) {
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

    // ---- 会话持久化(存储 + 读取,本轮不做自动恢复) ----

    /// 从当前播放上下文组装一份 [`SessionSnapshot`](锁不跨 await,调用方在锁内取完即用)。
    ///
    /// 队列存裸 [`SongId`] 保序;current 取 `current_song.id`;position / volume 读
    /// audio snapshot(`volume_pct` 0..=100 → `f64` 0.0..=1.0);play_mode 存 Debug 名稳定串。
    ///
    /// # Return:
    ///   组装好的 [`SessionSnapshot`]。
    fn snapshot_session(&self) -> SessionSnapshot {
        let audio = self.inner.audio.snapshot();
        let st = self.inner.state.lock();
        SessionSnapshot {
            current: st.current_song.as_ref().map(|s| s.id.clone()),
            position_ms: audio.position_ms,
            play_mode: play_mode_str(st.play_mode),
            volume: f64::from(audio.volume_pct) / 100.0,
            queue: st.queue.iter().map(|s| s.id.clone()).collect(),
        }
    }

    /// fire-and-forget 落盘当前会话:snapshot 在 spawn **前**组装好(锁不跨 await),
    /// owned move 进 task;失败仅 warn。降级 persist 下 save 自动 no-op。
    fn spawn_save_session(&self) {
        let snap = self.snapshot_session();
        let persist = self.inner.persist.clone();
        tokio::spawn(async move {
            if let Err(e) = persist.session().save(&snap).await {
                mineral_log::warn!(target: "player", error = mineral_log::chain(&e), "会话保存失败");
            }
        });
    }

    /// 读回上次会话快照(不应用到播放状态,本轮仅供启动日志确认能读到)。
    ///
    /// # Return:
    ///   上次会话;无历史 / 降级 persist 返回 `Ok(None)`。
    pub(crate) async fn load_session(&self) -> color_eyre::Result<Option<SessionSnapshot>> {
        self.inner.persist.session().load().await
    }

    // ---- 长跑后台 task ----

    /// 长跑后台 loop:每 tick 一次 events drain + harvest + auto-next + prefetch 检查。
    async fn background_loop(self) {
        let mut tick = tokio::time::interval(Duration::from_millis(TICK_INTERVAL_MS));
        loop {
            tick.tick().await;
            self.consume_events_once();
            self.check_harvest_ready();
            self.check_auto_next();
            self.check_prefetch();
            self.check_session_save();
        }
    }

    /// harvest:当前曲的远端字节一下完(engine `download_complete`)就把 capture 文件入缓存,
    /// **不等播放结束**。已 harvest(capturing 被取走)后再 tick 是 no-op。
    fn check_harvest_ready(&self) {
        if !self.inner.audio.snapshot().download_complete {
            return;
        }
        let cap = self.inner.state.lock().capturing.take();
        if let Some(cap) = cap {
            download::spawn_harvest(self, cap);
        }
    }

    /// 节流落盘:距上次周期 save 超过 [`SESSION_SAVE_INTERVAL`] 才 save 一次(主要刷新 position)。
    /// 状态变化类 save 走各自的即时 [`Self::spawn_save_session`],此处只补周期进度。
    fn check_session_save(&self) {
        {
            let mut last = self.inner.last_session_save.lock();
            if last.elapsed() < SESSION_SAVE_INTERVAL {
                return;
            }
            *last = Instant::now();
        }
        self.spawn_save_session();
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

    /// PlayUrlReady 命中当前歌 → audio.play + 写 play_url;命中已发起预拉的下一首 → 塞 prefetched;否则丢。
    fn handle_play_url_ready(&self, song_id: &SongId, play_url: PlayUrl) {
        let mut st = self.inner.state.lock();
        let want = st.current_song.as_ref().map(|t| &t.id);
        if want == Some(song_id) {
            mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "play", "play url ready");
            // 起播并 capture(供播完入缓存);需先放锁,helper 内部要再锁。
            let song = st.current_song.clone();
            st.play_url = Some(play_url.clone());
            drop(st);
            if let Some(song) = song {
                download::play_capturing(self, &song, &play_url, PLAYBACK_QUALITY);
            }
        } else if st.prefetch_fired_for.as_ref() == Some(song_id) {
            mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "prefetch", "play url ready");
            st.prefetched = Some(play_url);
        } else {
            // 用户已切到别的歌,丢。
            mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "drop", "play url ready");
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
        } else {
            // 非当前歌,无意义,丢(只缓存当前歌)。
            mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "drop", "lyrics ready");
        }
    }

    /// auto-next:audio engine 自然播完 → 按 PlayMode 切下一首。
    fn check_auto_next(&self) {
        let snap = self.inner.audio.snapshot();
        let last = self.inner.last_seen_finished_seq.load(Ordering::Relaxed);
        if snap.track_finished_seq <= last {
            return;
        }
        self.inner
            .last_seen_finished_seq
            .store(snap.track_finished_seq, Ordering::Relaxed);
        let (finished_id, mode, next) = {
            let st = self.inner.state.lock();
            (
                st.current_song.as_ref().map(|s| s.id.clone()),
                st.play_mode,
                next_in_queue(&st),
            )
        };
        // 自然播完:听了整首。duration 未知(decoder 未填)时退用 position。
        if let Some(finished) = finished_id.clone() {
            let listen_ms = if snap.duration_ms == 0 {
                snap.position_ms
            } else {
                snap.duration_ms
            };
            self.spawn_on_played(finished, /*completed*/ true, listen_ms);
        }
        if let Some(s) = next {
            mineral_log::info!(
                target: "player",
                finished_id = ?finished_id,
                next_id = s.id.as_str(),
                mode = ?mode,
                "auto next"
            );
            self.play_song(&s);
        }
    }

    /// prefetch:进度进入曲终前窗口时,submit 下一首 SongUrl(Background)。
    fn check_prefetch(&self) {
        let snap = self.inner.audio.snapshot();
        if snap.duration_ms == 0 {
            return;
        }
        if snap.duration_ms.saturating_sub(snap.position_ms) > PREFETCH_LEAD_MS {
            return;
        }
        let (cur_id, next) = {
            let st = self.inner.state.lock();
            let Some(cur) = st.current_song.as_ref() else {
                return;
            };
            if st.prefetch_fired_for.as_ref() == Some(&cur.id) {
                return;
            }
            (cur.id.clone(), next_in_queue(&st))
        };
        let Some(next) = next else {
            return;
        };
        self.inner.state.lock().prefetch_fired_for = Some(cur_id);
        mineral_log::debug!(target: "player", next_id = next.id.as_str(), source = ?next.source(), "prefetch next");
        self.inner.scheduler.submit(
            TaskKind::ChannelFetch(ChannelFetchKind::SongUrl {
                song_id: next.id,
                quality: PLAYBACK_QUALITY,
            }),
            Priority::Background,
        );
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
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    use async_trait::async_trait;
    use mineral_audio::{AudioHandle, AudioMode};
    use mineral_channel_core::{Error, MusicChannel, Page, Result as ChannelResult};
    use mineral_model::{
        Album, AlbumId, Artist, BitRate, Lyrics, PlayUrl, Playlist, PlaylistId, Song, SongId,
        SourceKind,
    };
    use mineral_persist::Persist;
    use mineral_protocol::PlayMode;
    use mineral_task::Scheduler;
    use mineral_test::song;
    use parking_lot::Mutex;
    use pretty_assertions::assert_eq;

    use super::{
        DownloadProgress, Inner, MediaCache, PlayerCore, State, apply_play_mode, enter_shuffle,
        exit_shuffle, next_in_queue, play_mode_str, prev_in_queue,
    };

    /// 记录型 mock channel:on_played 调用进 `calls`,其余方法返回 `NotSupported`。
    /// `source()` 报 `NETEASE`,与 [`mineral_test::song`] 的来源对齐,确保被路由命中。
    struct RecordingChannel {
        /// 已记录的 on_played 调用:(歌曲 id、是否完播、收听毫秒)。
        calls: Arc<Mutex<Vec<(SongId, bool, u64)>>>,
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
        core_with_persist(calls, Persist::disabled())
    }

    /// 同 [`core_with`],但允许注入指定 [`Persist`](会话持久化测试用真库)。
    ///
    /// # Params:
    ///   - `calls`: 共享的 on_played 调用记录。
    ///   - `persist`: 注入的持久化句柄。
    ///
    /// # Return:
    ///   组装好的 [`PlayerCore`]。
    fn core_with_persist(
        calls: Arc<Mutex<Vec<(SongId, bool, u64)>>>,
        persist: Persist,
    ) -> color_eyre::Result<PlayerCore> {
        let channel: Arc<dyn MusicChannel> = Arc::new(RecordingChannel { calls });
        let channels = vec![channel];
        let scheduler = Scheduler::new(&channels);
        let (audio, _tap) = AudioHandle::spawn(AudioMode::ForceNull)?;
        let inner = Arc::new(Inner {
            audio,
            scheduler,
            channels,
            persist,
            media_cache: Arc::new(MediaCache::disabled()),
            http: None,
            music_dir: None,
            download_progress: Arc::new(Mutex::new(DownloadProgress::default())),
            download_tx: tokio::sync::mpsc::unbounded_channel().0,
            download_pending: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            state: Mutex::new(State::empty()),
            last_seen_finished_seq: AtomicU64::new(0),
            client_events: Mutex::new(Vec::new()),
            last_session_save: Mutex::new(std::time::Instant::now()),
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
        let persist = Persist::open(&dir.path().join("t.db")).await?;
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
        let persist = Persist::open(&dir.path().join("t.db")).await?;
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

    /// fire-and-forget 的 spawn_save_session 最终能让 load 读到会话(不断言精确字段值,
    /// 只确认接线打通、数据落盘)。
    #[tokio::test]
    async fn spawn_save_session_persists_something() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = Persist::open(&dir.path().join("t.db")).await?;
        let calls = Arc::new(Mutex::new(Vec::<(SongId, bool, u64)>::new()));
        let core = core_with_persist(calls, persist)?;

        core.set_queue(vec![song("a"), song("b")], &song("a").id);
        drain_spawned().await;

        assert!(core.load_session().await?.is_some(), "save 后应能读到会话");
        Ok(())
    }
}
