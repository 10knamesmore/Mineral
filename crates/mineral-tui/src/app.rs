//! 顶层 [`App`] 状态与同步主事件循环。

use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use mineral_audio::{AudioHandle, SpectrumTap};
use mineral_model::{Song, SongId};
use mineral_task::{ChannelFetchKind, Priority, Scheduler, TaskEvent, TaskKind};

use crate::state::{AppState, Focus, View};
use crate::theme::Theme;
use crate::tui::Tui;
use crate::view::draw;

/// 启动期前 N 个歌单走 [`Priority::User`],之后走 [`Priority::Background`]——
/// "可见区域优先"的 hack 实现(终端再高也到不了 64 行)。
const VISIBLE_HINT: usize = 64;

/// 音量步长(百分点);`+`/`-` 一次。
const VOLUME_STEP: i16 = 5;

/// 普通 seek 步长(秒);`←`/`→` 一次。
const SEEK_STEP_S: i64 = 5;

/// 大跨度 seek 步长(秒);`Shift+←`/`Shift+→` 一次。
const SEEK_BIG_STEP_S: i64 = 30;

/// 大跨度跳行步长(行);`Shift+J`/`Shift+K` 一次。j/k/箭头仍是 1。
const ROW_BIG_STEP: usize = 7;

/// auto-next 预拉触发距曲终的剩余时间(ms)。播放进度进入此窗口就开始拉下一首的
/// SongUrl;5s 足够 SongUrl 200-500ms + stream-download 256KB prefetch_bytes 完成,
/// 又不至于早到 URL 过期。
const PREFETCH_LEAD_MS: u64 = 5_000;

/// `p` 键的「回开头 vs 上一首」分界(ms)。播放进度 > 阈值时按 p 回到本曲开头,
/// 否则跳上一首。3s 是 iTunes / Apple Music / Spotify 的默认行为,误触概率极低。
const PREV_RESTART_THRESHOLD_MS: u64 = 3_000;

/// 应用顶层状态。
pub struct App {
    /// 是否退出主循环。
    pub should_quit: bool,

    /// 当前主题。
    pub theme: Theme,

    /// 业务状态(视图、选中、playback、加载缓存等)。
    pub state: AppState,

    /// 上一次 tick 时间。
    pub last_tick: Instant,

    /// 任务调度器;UI 通过它提交 / 取消任务、拉事件。
    scheduler: Scheduler,

    /// 音频引擎 handle;所有播放控制走它。
    audio: AudioHandle,

    /// PCM tap:每 tick 拉一批样本喂给 [`crate::state::AppState::fft`]。
    spectrum_tap: SpectrumTap,

    /// 上次看到的 `AudioSnapshot::track_finished_seq`。tick 中检测到增长就触发
    /// auto-next。重置时机:`submit_play_song` 时同步对齐到当前 engine seq,
    /// 避免「新歌一上来就被旧 seq 误触发」。
    last_seen_finished_seq: u64,

    /// 已对哪首歌触发过预拉(避免同一首曲终前 tick 反复提交 SongUrl)。
    /// `submit_play_song` 时重置,新歌可以再次预拉它的下一首。
    prefetch_fired_for: Option<SongId>,
}

impl App {
    /// 构造 App。
    ///
    /// # Params:
    ///   - `scheduler`: 已构造的调度器,UI 持有共享引用
    ///   - `audio`: 已启动的音频引擎句柄
    ///   - `spectrum_tap`: 音频引擎吐出的 PCM 旁路
    pub fn new(scheduler: Scheduler, audio: AudioHandle, spectrum_tap: SpectrumTap) -> Self {
        Self {
            should_quit: false,
            theme: Theme::default(),
            state: AppState::empty(),
            last_tick: Instant::now(),
            scheduler,
            audio,
            spectrum_tap,
            last_seen_finished_seq: 0,
            prefetch_fired_for: None,
        }
    }

    /// 同步主事件循环:绘制 → 等事件 / tick → 处理 → 重绘。
    pub fn run(&mut self, tui: &mut Tui) -> color_eyre::Result<()> {
        // 33ms ≈ 30fps:yrc 字符级 wipe 渐变需要逐帧重算颜色,250ms 看上去会一跳一跳。
        // 终端渲染开销很小,30fps 在常规歌词长度下 <5% CPU,够顺滑。
        let tick_rate = Duration::from_millis(33);

        while !self.should_quit {
            self.drain_task_events();
            tui.draw(|f| draw(f, self))?;

            let timeout = tick_rate.saturating_sub(self.last_tick.elapsed());
            if event::poll(timeout)? {
                self.handle_event(&event::read()?);
            }
            if self.last_tick.elapsed() >= tick_rate {
                let snap = self.audio.snapshot();
                self.state.playback.apply_audio_snapshot(snap);
                self.update_spectrum();
                if snap.track_finished_seq > self.last_seen_finished_seq {
                    self.last_seen_finished_seq = snap.track_finished_seq;
                    self.advance_after_track_end();
                }
                self.maybe_submit_prefetch();
                self.last_tick = Instant::now();
            }
        }
        Ok(())
    }

    /// 把 audio 端 ringbuf 里的样本喂给 fft computer,算一窗,再喂给 spectrum 平滑器。
    /// pop chunk 比一帧产出量(48kHz × 33ms ≈ 1584)略大,确保单 tick 能 drain。
    fn update_spectrum(&mut self) {
        const POP_CHUNK: usize = 2048;
        let mut buf = [0_f32; POP_CHUNK];
        let n = self.spectrum_tap.pop_into(&mut buf);
        if let Some(slice) = buf.get(..n) {
            self.state.fft.push(slice);
        }
        let bars = self.state.fft.compute(self.spectrum_tap.sample_rate());
        self.state.spectrum.tick(
            self.state.playback.playing,
            self.state.playback.volume_pct,
            bars.as_ref(),
        );
    }

    fn drain_task_events(&mut self) {
        let events = self.scheduler.drain_events();
        for ev in &events {
            self.state.apply(ev);
            match ev {
                TaskEvent::PlaylistsFetched { playlists, .. } => {
                    self.fanout_playlist_tracks(playlists);
                }
                TaskEvent::PlayUrlReady { song_id, play_url } => {
                    let want = self.state.playback.track.as_ref().map(|t| &t.id);
                    if want == Some(song_id) {
                        // 当前歌的 URL 来了,直接播。
                        self.audio.play(play_url.url.clone());
                    } else if Some(song_id) == self.prefetch_fired_for.as_ref() {
                        // 预拉的 URL 命中:存进 cache,曲终切歌时直接用。
                        self.state.prefetched = Some((song_id.clone(), play_url.url.clone()));
                    }
                    // 其他 song_id:用户已切到别的歌或换了模式,旧 URL 直接丢。
                }
                TaskEvent::PlaylistTracksFetched { .. } | TaskEvent::LyricsReady { .. } => {}
            }
        }
    }

    /// Enter 一首歌:杀掉旧 PlayPrep + Lyrics 任务、audio.stop,然后提交新的
    /// PlayPrep + Lyrics(都 User 优先级)。收到 `PlayUrlReady` 后才真正出声。
    /// 同步把 `queue_sel` 对齐到这首歌在 queue 里的位置(不在则保持原值),
    /// 让后续 prev/next/auto-next 有正确锚点。
    fn submit_play_song(&mut self, song: &Song) {
        self.scheduler.cancel_where(|k| {
            matches!(
                k,
                TaskKind::ChannelFetch(
                    ChannelFetchKind::SongUrl { .. } | ChannelFetchKind::Lyrics { .. }
                )
            )
        });
        self.audio.stop();
        self.state.current = Some(song.clone());
        self.state.playback.track = Some(song.clone());
        self.state.playback.position_ms = 0;
        if let Some(idx) = self.state.queue.iter().position(|s| s.id == song.id) {
            self.state.queue_sel = idx;
        }
        // 把 last_seen_finished_seq 对齐到当前 engine seq —— audio.stop() 不会
        // 触发曲终,但避免极端情况下旧 seq 比新 snapshot 大造成误触发。
        self.last_seen_finished_seq = self.audio.snapshot().track_finished_seq;
        // 新歌允许再次预拉它的下一首。
        self.prefetch_fired_for = None;

        // 命中 prefetch:跳过 SongUrl 提交,直接把缓存的 URL 喂 audio。
        let cached = match self.state.prefetched.take() {
            Some((id, url)) if id == song.id => Some(url),
            other => {
                // take 拿走后失配的 URL 已无人认领,丢掉(不放回)。
                drop(other);
                None
            }
        };
        if let Some(url) = cached {
            self.audio.play(url);
        } else {
            self.scheduler.submit(
                TaskKind::ChannelFetch(ChannelFetchKind::SongUrl {
                    source: song.source,
                    song_id: song.id.clone(),
                }),
                Priority::User,
            );
        }
        self.scheduler.submit(
            TaskKind::ChannelFetch(ChannelFetchKind::Lyrics {
                source: song.source,
                song_id: song.id.clone(),
            }),
            Priority::User,
        );
    }

    /// 播放进度进入曲终前 [`PREFETCH_LEAD_MS`] 窗口时,提交下一首的 SongUrl(Background)。
    /// 每首歌只触发一次。Shuffle 现在也走顺序 queue,next 可预测,不再排除。
    fn maybe_submit_prefetch(&mut self) {
        let pb = &self.state.playback;
        let Some(cur_song) = pb.track.as_ref() else {
            return;
        };
        let dur = pb.duration_ms();
        if dur == 0 {
            return;
        }
        if dur.saturating_sub(pb.position_ms) > PREFETCH_LEAD_MS {
            return;
        }
        if self.prefetch_fired_for.as_ref() == Some(&cur_song.id) {
            return;
        }
        let Some(next) = self.next_song() else {
            return;
        };
        self.prefetch_fired_for = Some(cur_song.id.clone());
        self.scheduler.submit(
            TaskKind::ChannelFetch(ChannelFetchKind::SongUrl {
                source: next.source,
                song_id: next.id,
            }),
            Priority::Background,
        );
    }

    /// 替换 queue:非 Shuffle 直接赋 + 按 target_id 设 queue_sel;Shuffle 则保存原序、
    /// 洗牌、把 target swap 到 index 0。统一入口避免 Enter 新歌单 / 后续切歌需要在两种
    /// 状态下手动维护。
    fn set_queue(&mut self, new_queue: Vec<Song>, target_id: &SongId) {
        if matches!(self.state.playback.mode, crate::playback::PlayMode::Shuffle) {
            use rand::seq::SliceRandom;
            let mut shuffled = new_queue.clone();
            shuffled.shuffle(&mut rand::rng());
            // target swap 到 index 0,保证当前歌在 queue 顶。
            if let Some(pos) = shuffled.iter().position(|s| s.id == *target_id) {
                shuffled.swap(0, pos);
            }
            self.state.original_queue = Some(new_queue);
            self.state.queue = shuffled;
            self.state.queue_sel = 0;
        } else {
            let sel = new_queue
                .iter()
                .position(|s| s.id == *target_id)
                .unwrap_or(0);
            self.state.queue = new_queue;
            self.state.queue_sel = sel;
            self.state.original_queue = None;
        }
    }

    /// `m` 键循环播放模式;在进 / 退 Shuffle 边界处洗牌或还原。
    fn cycle_play_mode(&mut self) {
        use crate::playback::PlayMode;
        let old = self.state.playback.mode;
        let new = old.cycle();
        self.state.playback.mode = new;
        match (old, new) {
            (m1, PlayMode::Shuffle) if m1 != PlayMode::Shuffle => self.enter_shuffle(),
            (PlayMode::Shuffle, m2) if m2 != PlayMode::Shuffle => self.exit_shuffle(),
            _ => {}
        }
    }

    /// 进入 Shuffle:保存原序、洗牌、当前歌 swap 到 index 0、queue_sel = 0。
    /// 之后正向推进就是「当前歌之后随机播」。
    fn enter_shuffle(&mut self) {
        use rand::seq::SliceRandom;
        if self.state.queue.is_empty() {
            return;
        }
        let original = self.state.queue.clone();
        let cur_id = self.state.playback.track.as_ref().map(|t| t.id.clone());
        self.state.queue.shuffle(&mut rand::rng());
        if let Some(id) = cur_id {
            if let Some(pos) = self.state.queue.iter().position(|s| s.id == id) {
                self.state.queue.swap(0, pos);
            }
        }
        self.state.queue_sel = 0;
        self.state.original_queue = Some(original);
    }

    /// 退出 Shuffle:还原原序,queue_sel 落在当前歌在原序中的位置。
    fn exit_shuffle(&mut self) {
        let Some(original) = self.state.original_queue.take() else {
            return;
        };
        let cur_id = self.state.playback.track.as_ref().map(|t| t.id.clone());
        self.state.queue = original;
        self.state.queue_sel = cur_id
            .and_then(|id| self.state.queue.iter().position(|s| s.id == id))
            .unwrap_or(0);
    }

    /// 一首自然播完后按 PlayMode 选下一首播。Sequential 到末尾就停。
    fn advance_after_track_end(&mut self) {
        if let Some(next) = self.next_song() {
            self.submit_play_song(&next);
        }
    }

    /// 按 PlayMode 选下一首。queue 空 / Sequential 到末尾返回 `None`。
    /// Shuffle 这里跟 RepeatAll 一样按 queue 顺序推 + wrap —— queue 在进 Shuffle 时
    /// 已经一次性洗过(见 [`Self::enter_shuffle`]),next 是确定性的。
    fn next_song(&self) -> Option<Song> {
        use crate::playback::PlayMode;
        let len = self.state.queue.len();
        if len == 0 {
            return None;
        }
        let cur = self.state.queue_sel.min(len - 1);
        match self.state.playback.mode {
            PlayMode::Sequential => self.state.queue.get(cur + 1).cloned(),
            PlayMode::RepeatAll | PlayMode::Shuffle => {
                self.state.queue.get((cur + 1) % len).cloned()
            }
            PlayMode::RepeatOne => self.state.queue.get(cur).cloned(),
        }
    }

    /// `p` 键行为:进度 > [`PREV_RESTART_THRESHOLD_MS`] 时回到本曲开头,
    /// 否则跳上一首(对齐 iTunes / Spotify)。无 track 时直接 no-op。
    fn prev_or_restart(&mut self) {
        if self.state.playback.track.is_none() {
            return;
        }
        if self.state.playback.position_ms > PREV_RESTART_THRESHOLD_MS {
            self.audio.seek(0);
            return;
        }
        if let Some(s) = self.prev_song() {
            self.submit_play_song(&s);
        }
    }

    /// 按 PlayMode 选上一首。Sequential 到队首返回 `None`。Shuffle 在洗过的 queue 里
    /// 顺序回退,真能回到「刚播过的那首」(不再是再随机)。
    fn prev_song(&self) -> Option<Song> {
        use crate::playback::PlayMode;
        let len = self.state.queue.len();
        if len == 0 {
            return None;
        }
        let cur = self.state.queue_sel.min(len - 1);
        match self.state.playback.mode {
            PlayMode::Sequential => {
                if cur == 0 {
                    None
                } else {
                    self.state.queue.get(cur - 1).cloned()
                }
            }
            PlayMode::RepeatAll | PlayMode::Shuffle => {
                self.state.queue.get((cur + len - 1) % len).cloned()
            }
            PlayMode::RepeatOne => self.state.queue.get(cur).cloned(),
        }
    }

    /// 收到歌单批次后,逐条提交 `PlaylistTracks` 任务。前 [`VISIBLE_HINT`] 条
    /// 用 User 优先级,余下用 Background;dedup 在 scheduler 内自动处理。
    fn fanout_playlist_tracks(&self, playlists: &[mineral_model::Playlist]) {
        let already_loaded = self.state.playlists.len().saturating_sub(playlists.len());
        for (offset, p) in playlists.iter().enumerate() {
            let idx = already_loaded + offset;
            let priority = if idx < VISIBLE_HINT {
                Priority::User
            } else {
                Priority::Background
            };
            self.scheduler.submit(
                TaskKind::ChannelFetch(ChannelFetchKind::PlaylistTracks {
                    source: p.source,
                    id: p.id.clone(),
                }),
                priority,
            );
        }
    }

    fn handle_event(&mut self, ev: &Event) {
        if let Event::Key(key) = ev {
            if key.kind == KeyEventKind::Press {
                self.handle_key(key);
            }
        }
    }

    fn handle_key(&mut self, key: &KeyEvent) {
        // Ctrl-C 强制退出(skip confirm)。
        if matches!(
            (key.modifiers, key.code),
            (KeyModifiers::CONTROL, KeyCode::Char('c'))
        ) {
            self.should_quit = true;
            return;
        }

        // 最高 UI 优先级:搜索输入态吞掉所有键。
        if self.state.search_mode {
            self.handle_search_key(key);
            return;
        }

        // 其次:confirm modal。
        if self.state.confirm_open {
            self.handle_confirm_key(key);
            return;
        }

        // Tab 切换 queue 浮层。
        if key.code == KeyCode::Tab {
            self.toggle_queue();
            return;
        }

        // q:queue 打开时关闭 queue,否则打开 quit confirm。
        if key.code == KeyCode::Char('q') {
            if self.state.queue_open {
                self.close_queue();
            } else {
                self.state.confirm_open = true;
            }
            return;
        }

        // queue focused 时,Esc 关闭 queue;playback 全局键(空格 / n / p / m / 音量 /
        // seek)优先消费,余下走 queue 内导航(j/k/Enter 等)。
        if self.state.focus == Focus::Queue {
            if key.code == KeyCode::Esc {
                self.close_queue();
                return;
            }
            if self.handle_playback_key(key) {
                return;
            }
            self.handle_queue_key(key);
            return;
        }

        // / 进入搜索输入态(直接编辑 search_q)。
        if key.code == KeyCode::Char('/') {
            self.state.search_mode = true;
            self.state.search_q.clear();
            return;
        }

        // 全局 playback 键(Space / m / +- / ←→ / p / n)。
        if self.handle_playback_key(key) {
            return;
        }

        // 视图分派。
        match self.state.view {
            View::Playlists => self.handle_playlists_key(key),
            View::Library => self.handle_library_key(key),
        }
    }

    fn handle_confirm_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
                self.should_quit = true;
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                self.state.confirm_open = false;
            }
            _ => {}
        }
    }

    fn handle_search_key(&mut self, key: &KeyEvent) {
        match key.code {
            // Esc: 清掉过滤词并退出输入态。
            KeyCode::Esc => {
                self.state.search_mode = false;
                self.state.search_q.clear();
            }
            // Enter: 退出输入态,过滤词保留继续生效。
            KeyCode::Enter => {
                self.state.search_mode = false;
            }
            KeyCode::Backspace => {
                self.state.search_q.pop();
                self.state.sel_playlist = 0;
                self.state.sel_track = 0;
            }
            KeyCode::Char(c) => {
                self.state.search_q.push(c);
                self.state.sel_playlist = 0;
                self.state.sel_track = 0;
            }
            _ => {}
        }
    }

    fn toggle_queue(&mut self) {
        self.state.queue_open = !self.state.queue_open;
        self.state.focus = if self.state.queue_open {
            Focus::Queue
        } else {
            Focus::Left
        };
    }

    fn close_queue(&mut self) {
        self.state.queue_open = false;
        self.state.focus = Focus::Left;
    }

    fn handle_queue_key(&mut self, key: &KeyEvent) {
        let len = self.state.queue.len();
        let max = len.saturating_sub(1);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.state.queue_sel = self.state.queue_sel.saturating_add(1).min(max);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.state.queue_sel = self.state.queue_sel.saturating_sub(1);
            }
            KeyCode::Char('J') => {
                self.state.queue_sel = self.state.queue_sel.saturating_add(ROW_BIG_STEP).min(max);
            }
            KeyCode::Char('K') => {
                self.state.queue_sel = self.state.queue_sel.saturating_sub(ROW_BIG_STEP);
            }
            KeyCode::Char('g') => {
                self.state.queue_sel = 0;
            }
            KeyCode::Char('G') => {
                self.state.queue_sel = len.saturating_sub(1);
            }
            KeyCode::Enter => {
                if let Some(s) = self.state.queue.get(self.state.queue_sel).cloned() {
                    self.submit_play_song(&s);
                }
            }
            _ => {}
        }
    }

    fn handle_playback_key(&mut self, key: &KeyEvent) -> bool {
        // Shift+←/→ 走大跨度 seek;终端通常会原样把 SHIFT modifier 透出。
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::Left => {
                    self.seek_relative(-SEEK_BIG_STEP_S);
                    return true;
                }
                KeyCode::Right => {
                    self.seek_relative(SEEK_BIG_STEP_S);
                    return true;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Char(' ') => self.toggle_play_pause(),
            KeyCode::Char('m') => self.cycle_play_mode(),
            KeyCode::Char('+') | KeyCode::Char('=') => self.nudge_volume(VOLUME_STEP),
            KeyCode::Char('-') | KeyCode::Char('_') => self.nudge_volume(-VOLUME_STEP),
            KeyCode::Left => self.seek_relative(-SEEK_STEP_S),
            KeyCode::Right => self.seek_relative(SEEK_STEP_S),
            KeyCode::Char('p') => self.prev_or_restart(),
            KeyCode::Char('n') => {
                if let Some(s) = self.next_song() {
                    self.submit_play_song(&s);
                }
            }
            _ => return false,
        }
        true
    }

    fn toggle_play_pause(&mut self) {
        if self.state.playback.track.is_none() {
            return;
        }
        if self.state.playback.playing {
            self.audio.pause();
        } else {
            self.audio.resume();
        }
    }

    fn nudge_volume(&mut self, delta: i16) {
        let cur = i16::from(self.state.playback.volume_pct);
        let new = cur.saturating_add(delta).clamp(0, 100);
        let pct = u8::try_from(new).unwrap_or(self.state.playback.volume_pct);
        self.audio.set_volume(pct);
        self.state.playback.volume_pct = pct;
    }

    fn seek_relative(&mut self, delta_s: i64) {
        let dur_ms = self.state.playback.duration_ms();
        if dur_ms == 0 {
            return;
        }
        let cur = i64::try_from(self.state.playback.position_ms).unwrap_or(0);
        let max = i64::try_from(dur_ms).unwrap_or(0);
        let new_ms = cur
            .saturating_add(delta_s.saturating_mul(1000))
            .clamp(0, max);
        let new_u = u64::try_from(new_ms).unwrap_or(0);
        self.audio.seek(new_u);
    }

    fn handle_playlists_key(&mut self, key: &KeyEvent) {
        let max = self.state.playlists.len().saturating_sub(1);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.state.sel_playlist = self.state.sel_playlist.saturating_add(1).min(max);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.state.sel_playlist = self.state.sel_playlist.saturating_sub(1);
            }
            KeyCode::Char('J') => {
                self.state.sel_playlist = self
                    .state
                    .sel_playlist
                    .saturating_add(ROW_BIG_STEP)
                    .min(max);
            }
            KeyCode::Char('K') => {
                self.state.sel_playlist = self.state.sel_playlist.saturating_sub(ROW_BIG_STEP);
            }
            KeyCode::Char('g') => {
                self.state.sel_playlist = 0;
            }
            KeyCode::Char('G') => {
                self.state.sel_playlist = self.state.playlists.len().saturating_sub(1);
            }
            KeyCode::Char('l') | KeyCode::Enter => {
                self.state.view = View::Library;
                self.state.sel_track = 0;
            }
            _ => {}
        }
    }

    fn handle_library_key(&mut self, key: &KeyEvent) {
        let len = self.state.current_tracks().len();
        let max = len.saturating_sub(1);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.state.sel_track = self.state.sel_track.saturating_add(1).min(max);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.state.sel_track = self.state.sel_track.saturating_sub(1);
            }
            KeyCode::Char('J') => {
                self.state.sel_track = self.state.sel_track.saturating_add(ROW_BIG_STEP).min(max);
            }
            KeyCode::Char('K') => {
                self.state.sel_track = self.state.sel_track.saturating_sub(ROW_BIG_STEP);
            }
            KeyCode::Char('g') => {
                self.state.sel_track = 0;
            }
            KeyCode::Char('G') => {
                self.state.sel_track = len.saturating_sub(1);
            }
            KeyCode::Char('h') | KeyCode::Esc | KeyCode::Backspace => {
                self.state.view = View::Playlists;
            }
            KeyCode::Enter => {
                let tracks = self.state.current_tracks();
                if let Some(s) = tracks.get(self.state.sel_track).map(|sv| sv.data.clone()) {
                    let new_queue: Vec<Song> = tracks.into_iter().map(|sv| sv.data).collect();
                    self.set_queue(new_queue, &s.id);
                    self.submit_play_song(&s);
                }
            }
            _ => {}
        }
    }
}
