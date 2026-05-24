//! 服务端 PlayerCore — 集中持有「播放上下文」(current_song / queue / play_mode /
//! play_url / current_lyrics / prefetched 等),让 daemon 自治 auto-next、不依赖 client。
//!
//! 历史上这些状态全在 client(`mineral-tui::AppState`),client 关闭就丢光,daemon
//! 不知道当前在播哪首歌、不会自动切下一首。4c 把数据 + 业务流程整体搬过来,
//! `App` 退化为「转发 + 渲染」。
//!
//! ## 流程契约
//!
//! - `play_song(song)`: client 选了一首歌 — server cancel 旧 SongUrl/Lyrics、
//!   audio.stop、记 current_song、命中 prefetched 直接 audio.play、否则 submit
//!   新 SongUrl + Lyrics 任务。
//! - long-running event task: 周期 drain scheduler events,**消化** PlayUrlReady /
//!   LyricsReady (按 current_song 配对、必要时 audio.play、塞 lyrics cache),
//!   其它 events (PlaylistsFetched / PlaylistTracksFetched / LikedSongIdsFetched)
//!   原样推到 [`PlayerCore::drain_client_events`] 供 client 拉。
//! - long-running auto-next task: 监听 `audio_snapshot.track_finished_seq`,
//!   增长就 `advance_after_track_end()`(按 PlayMode 选下一首 + play_song)。
//! - long-running prefetch task: 进度进入 `PREFETCH_LEAD_MS` 窗口时 submit
//!   下一首 SongUrl,拿到塞 prefetched cache。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mineral_audio::AudioHandle;
use mineral_channel_core::MusicChannel;
use mineral_model::{BitRate, PlayUrl, Song, SongId, SourceKind};
use mineral_protocol::{PlayMode, PlayerSnapshot};
use mineral_task::{ChannelFetchKind, Priority, Scheduler, Snapshot, TaskEvent, TaskId, TaskKind};
use parking_lot::Mutex;
use rand::seq::SliceRandom;

/// 播放音质。后续接 config 时改读配置。
const PLAYBACK_QUALITY: BitRate = BitRate::Lossless;

/// auto-next 预拉触发距曲终的剩余时间(ms)。
const PREFETCH_LEAD_MS: u64 = 5_000;

/// `p` 键的「回开头 vs 上一首」分界(ms)。
const PREV_RESTART_THRESHOLD_MS: u64 = 3_000;

/// 长跑 task 的醒来间隔。20ms 远小于 client 30fps tick,事件最坏延迟 ~50ms。
const TICK_INTERVAL_MS: u64 = 20;

/// 服务端持有的「播放上下文」内部状态。`PlayerCore` 用 `Mutex<State>` 包它。
struct State {
    /// 当前在播的歌。
    current_song: Option<Song>,

    /// 当前歌的播放 URL(从 SongUrlReady 写入)。
    play_url: Option<PlayUrl>,

    /// 当前队列(顺序模式 = 原序;shuffle 模式 = 洗过)。
    queue: Vec<Song>,

    /// 当前歌在 `queue` 中的下标。
    queue_sel: usize,

    /// shuffle 切换前的原始顺序,关 shuffle 时还原用;非 shuffle 模式下为 `None`。
    original_queue: Option<Vec<Song>>,

    /// 当前播放模式(顺序 / 单曲 / 列表循环 / shuffle)。
    play_mode: PlayMode,

    /// 当前歌的歌词(从 LyricsReady 写入)。
    current_lyrics: Option<mineral_model::Lyrics>,

    /// 当前 lyrics 配对的歌 id(对不上 current_song 时不返回)。
    current_lyrics_song_id: Option<SongId>,

    /// 已预拉的下一首播放 URL。
    prefetched: Option<PlayUrl>,

    /// 已对哪首歌触发过预拉(本歌只 fire 一次)。
    prefetch_fired_for: Option<SongId>,
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

    /// 播放上下文(队列/当前歌/歌词/预拉状态)。
    state: Mutex<State>,

    /// 已转发给 client 的最新 finished seq;auto-next 监听它。
    last_seen_finished_seq: AtomicU64,

    /// PlayUrlReady/LyricsReady 之外的 events 暂存,client drain 时取走。
    client_events: Mutex<Vec<TaskEvent>>,
}

impl PlayerCore {
    /// 起 PlayerCore 并 spawn 长跑 task(events drain + auto-next + prefetch tick)。
    pub fn spawn(
        audio: AudioHandle,
        scheduler: Scheduler,
        channels: Vec<Arc<dyn MusicChannel>>,
    ) -> Self {
        let inner = Arc::new(Inner {
            audio,
            scheduler,
            channels,
            state: Mutex::new(State::empty()),
            last_seen_finished_seq: AtomicU64::new(0),
            client_events: Mutex::new(Vec::new()),
        });
        let me = Self { inner };
        let bg = me.clone();
        tokio::spawn(async move { bg.background_loop().await });
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
        self.inner.audio.stop();

        let cached_url = {
            let mut st = self.inner.state.lock();
            st.current_song = Some(song.clone());
            if let Some(idx) = st.queue.iter().position(|s| s.id == song.id) {
                st.queue_sel = idx;
            }
            st.play_url = None;
            st.current_lyrics = None;
            st.current_lyrics_song_id = None;
            st.prefetch_fired_for = None;
            // 命中 prefetch 的 take 出来,留 None。
            match st.prefetched.take() {
                Some(pu) if pu.song_id == song.id => Some(pu),
                _ => None,
            }
        };
        // 对齐 finished_seq,防止 audio.stop() 极端时序下被旧 seq 误触发。
        let seq = self.inner.audio.snapshot().track_finished_seq;
        self.inner
            .last_seen_finished_seq
            .store(seq, Ordering::Relaxed);

        if let Some(pu) = cached_url {
            mineral_log::debug!(target: "player", song_id = song.id.as_str(), "using prefetched url");
            self.inner.audio.play(pu.url.clone());
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
    }

    /// 替换 queue。等价历史 `App::set_queue`。
    pub fn set_queue(&self, new_queue: Vec<Song>, target_id: &SongId) {
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

    /// `m` 键:PlayMode cycle + 进/退 Shuffle 边界处洗牌或还原。
    pub fn cycle_play_mode(&self) {
        let mut st = self.inner.state.lock();
        let new = st.play_mode.cycle();
        apply_play_mode(&mut st, new);
    }

    /// 直接设目标 PlayMode(系统媒体控件按维度写 Shuffle/LoopStatus 后塌缩成的档)。
    pub fn set_play_mode(&self, mode: PlayMode) {
        let mut st = self.inner.state.lock();
        apply_play_mode(&mut st, mode);
    }

    /// `p` 键:进度 > 阈值 → seek(0);否则跳上一首。
    pub fn prev_or_restart(&self) {
        let pos = self.inner.audio.snapshot().position_ms;
        let prev = {
            let st = self.inner.state.lock();
            if st.current_song.is_none() {
                return;
            }
            if pos > PREV_RESTART_THRESHOLD_MS {
                drop(st);
                self.inner.audio.seek(0);
                return;
            }
            prev_in_queue(&st)
        };
        if let Some(s) = prev {
            self.play_song(&s);
        }
    }

    /// `n` 键:按 PlayMode 切下一首。
    pub fn next_song(&self) {
        let next = next_in_queue(&self.inner.state.lock());
        if let Some(s) = next {
            self.play_song(&s);
        }
    }

    // ---- 长跑后台 task ----

    /// 长跑后台 loop:每 tick 一次 events drain + auto-next + prefetch 检查。
    async fn background_loop(self) {
        let mut tick = tokio::time::interval(Duration::from_millis(TICK_INTERVAL_MS));
        loop {
            tick.tick().await;
            self.consume_events_once();
            self.check_auto_next();
            self.check_prefetch();
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

    /// PlayUrlReady 命中当前歌 → audio.play + 写 play_url;命中已发起预拉的下一首 → 塞 prefetched;否则丢。
    fn handle_play_url_ready(&self, song_id: &SongId, play_url: PlayUrl) {
        let mut st = self.inner.state.lock();
        let want = st.current_song.as_ref().map(|t| &t.id);
        if want == Some(song_id) {
            mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "play", "play url ready");
            self.inner.audio.play(play_url.url.clone());
            st.play_url = Some(play_url);
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

/// 按 [`PlayMode`] 计算「下一首」:Sequential 到尾返回 None,Repeat/Shuffle 环回 0,RepeatOne 原地。
fn next_in_queue(st: &State) -> Option<Song> {
    let len = st.queue.len();
    if len == 0 {
        return None;
    }
    let cur = st.queue_sel.min(len - 1);
    match st.play_mode {
        PlayMode::Sequential => st.queue.get(cur + 1).cloned(),
        PlayMode::RepeatAll | PlayMode::Shuffle => st.queue.get((cur + 1) % len).cloned(),
        PlayMode::RepeatOne => st.queue.get(cur).cloned(),
    }
}

/// 按 [`PlayMode`] 计算「上一首」:Sequential 在 0 时返回 None,Repeat/Shuffle 环回末尾,RepeatOne 原地。
fn prev_in_queue(st: &State) -> Option<Song> {
    let len = st.queue.len();
    if len == 0 {
        return None;
    }
    let cur = st.queue_sel.min(len - 1);
    match st.play_mode {
        PlayMode::Sequential => {
            if cur == 0 {
                None
            } else {
                st.queue.get(cur - 1).cloned()
            }
        }
        PlayMode::RepeatAll | PlayMode::Shuffle => st.queue.get((cur + len - 1) % len).cloned(),
        PlayMode::RepeatOne => st.queue.get(cur).cloned(),
    }
}

/// 占位函数,仅为了在本模块里"用一下" [`SourceKind`],避免 unused import 警告;
/// 等后续真正用 channel 元信息时删除。
#[allow(dead_code)]
fn _channels_meta_placeholder(_s: SourceKind) {}

#[cfg(test)]
mod tests {
    use mineral_model::Song;
    use mineral_protocol::PlayMode;
    use mineral_test::song;
    use pretty_assertions::assert_eq;

    use super::{
        State, apply_play_mode, enter_shuffle, exit_shuffle, next_in_queue, prev_in_queue,
    };

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
}
