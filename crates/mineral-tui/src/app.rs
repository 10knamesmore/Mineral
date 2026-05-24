//! 顶层 [`App`] 状态与同步主事件循环。
//!
//! **4c 重构后**:player 业务(submit_play_song / next/prev/cycle_mode / queue 管理 /
//! auto-next / prefetch)整体搬到 server (`mineral_server::PlayerCore`)。App 退化
//! 为「转发用户意图 + 渲染 server snapshot」。每帧 tick 拉一次 PlayerSnapshot 灌
//! 进 AppState 镜像;按键直接转 `client.play_song / cycle_play_mode / ...` 等
//! 高级意图。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use mineral_model::Song;
use mineral_protocol::PlayerSnapshot;
use mineral_server::Client;
use mineral_task::TaskEvent;
use ratatui_image::picker::Picker;

use crate::cover::CoverFetcher;
use crate::state::{AppState, Focus, View};
use crate::theme::Theme;
use crate::tui::Tui;
use crate::view::draw;

/// 音量步长(百分点);`+`/`-` 一次。
const VOLUME_STEP: i16 = 5;

/// 普通 seek 步长(秒);`←`/`→` 一次。
const SEEK_STEP_S: i64 = 5;

/// 大跨度 seek 步长(秒);`Shift+←`/`Shift+→` 一次。
const SEEK_BIG_STEP_S: i64 = 30;

/// 大跨度跳行步长(行);`Shift+J`/`Shift+K` 一次。j/k/箭头仍是 1。
const ROW_BIG_STEP: usize = 7;

/// 应用顶层状态。
pub struct App {
    /// 是否退出主循环。
    pub should_quit: bool,

    /// 当前主题。
    pub theme: Theme,

    /// 业务状态(视图、选中、playback 镜像、加载缓存等)。
    pub state: AppState,

    /// 上一次 tick 时间。
    pub last_tick: Instant,

    /// Server client:所有「调命令 / 拉 snapshot / 拉事件」都走它。
    /// 实现可能是同进程 `ClientHandle`,也可能是跨进程 `RemoteClient`,通过
    /// [`Client`] trait 抽象。**player 业务在 server 端**;App 只 forward 意图。
    client: Arc<dyn Client>,

    /// Client 端 cover fetcher。封面是 client-local 资源,不归 server 管。
    cover_fetcher: CoverFetcher,

    /// 终端图片协议探测结果。
    pub picker: Picker,
}

impl App {
    /// 构造 App。
    ///
    /// # Params:
    ///   - `client`: 跟 server 交互的句柄
    ///   - `cover_fetcher`: client 端 cover fetcher
    ///   - `picker`: 终端图片协议能力
    pub fn new(client: Arc<dyn Client>, cover_fetcher: CoverFetcher, picker: Picker) -> Self {
        Self {
            should_quit: false,
            theme: Theme::default(),
            state: AppState::empty(),
            last_tick: Instant::now(),
            client,
            cover_fetcher,
            picker,
        }
    }

    /// 同步主事件循环:绘制 → 等事件 / tick → 处理 → 重绘。
    pub fn run(&mut self, tui: &mut Tui) -> color_eyre::Result<()> {
        // 33ms ≈ 30fps。
        let tick_rate = Duration::from_millis(33);

        // 启动时拉一次 PlayerSnapshot,让 in-proc / connect 都立即看到 server 状态。
        self.apply_player_snapshot(self.client.player_snapshot());

        // client 侧 60s 心跳:报 server 看不到的 UI / 缓存状态(启动即首条)。
        let mut last_heartbeat = Instant::now();
        self.log_heartbeat();

        // 退出信号 watcher:SIGTERM / SIGINT / SIGHUP 进来时不再 silent kill,而是由
        // 后台 task 记日志 + 置标志,主循环据此走正常退出(`Tui::exit` 还原终端)。
        let shutdown = crate::signal::spawn_watcher()?;

        while !self.should_quit {
            if shutdown.load(Ordering::Acquire) {
                self.should_quit = true;
                break;
            }
            // daemon 被单独 kill / crash → 链路断开。不僵死在「请求全兜底默认值」的
            // 状态:置断连态(记一条 error),进入下面的「显示话术 + 等按键退出」分支。
            if !self.state.disconnected && !self.client.connected() {
                mineral_log::error!(target: "tui", "daemon connection lost, awaiting key to exit");
                self.state.disconnected = true;
            }
            if self.state.disconnected {
                // 只渲染断连提示 + 等按键;daemon 没了,正常路径全是兜底默认值,跳过。
                tui.draw(|f| draw(f, self))?;
                if event::poll(tick_rate)?
                    && let Event::Key(key) = event::read()?
                    && key.kind == KeyEventKind::Press
                {
                    self.should_quit = true;
                }
                continue;
            }
            self.drain_task_events();
            tui.draw(|f| draw(f, self))?;

            let timeout = tick_rate.saturating_sub(self.last_tick.elapsed());
            if event::poll(timeout)? {
                self.handle_event(&event::read()?);
            }
            if self.last_tick.elapsed() >= tick_rate {
                let snap = self.client.audio_snapshot();
                self.state.playback.apply_audio_snapshot(snap);
                self.update_spectrum();
                self.apply_player_snapshot(self.client.player_snapshot());
                self.drain_ready_covers();
                crate::prefetch::tick(&mut self.state, &*self.client, &self.cover_fetcher);
                self.state.tasks_snapshot = self.client.task_snapshot();
                self.state.cover_loading = self.state.cover_pending.len();
                self.last_tick = Instant::now();
                if last_heartbeat.elapsed() >= Duration::from_secs(60) {
                    self.log_heartbeat();
                    last_heartbeat = Instant::now();
                }
            }
        }
        Ok(())
    }

    /// client 侧心跳:把 server 看不到的 UI / 缓存状态打一条 info。大缓存
    /// (tracks / cover / lyrics)都在 client 端,server 心跳报不了,这里补上。
    fn log_heartbeat(&self) {
        let s = &self.state;
        let liked = s
            .liked_ids
            .values()
            .fold(0_usize, |acc, set| acc + set.len());
        mineral_log::info!(
            target: "heartbeat",
            view = ?s.view,
            focus = ?s.focus,
            playlists = s.playlists.len(),
            tracks_cached = s.tracks_cache.len(),
            tracks_requested = s.tracks_requested.len(),
            lyrics_cached = s.lyrics_cache.len(),
            words_cached = s.words_cache.len(),
            covers_cached = s.cover_cache.len(),
            covers_pending = s.cover_pending.len(),
            liked,
            queue_len = s.queue.len(),
            "client status"
        );
    }

    /// 把 server 的 PlayerSnapshot 灌进 AppState 镜像 — current_song / queue /
    /// play_mode / play_url / current_lyrics 等。每帧调一次。
    fn apply_player_snapshot(&mut self, snap: PlayerSnapshot) {
        self.state.current = snap.current_song.clone();
        self.state.playback.track = snap.current_song;
        self.state.playback.play_url = snap.play_url;
        self.state.playback.mode = snap.play_mode;
        self.state.queue = snap.queue;
        self.state.queue_sel = snap.queue_sel;
        self.state.original_queue = snap.original_queue;
        // lyrics cache: 仅按 server 给的「current_lyrics_song_id」灌。歌词在 channel
        // 层已结构化清洗,这里直接收下结构化数据,不再解析。
        if let (Some(song_id), Some(lyrics)) = (snap.current_lyrics_song_id, snap.current_lyrics)
            && !self.state.lyrics_cache.contains_key(&song_id)
        {
            if !lyrics.words.is_empty() {
                self.state.words_cache.insert(song_id.clone(), lyrics.words);
            }
            self.state.lyrics_cache.insert(song_id, lyrics.lrc);
        }
    }

    /// 把 cover_fetcher 就绪的图写进 cache + 清掉对应 protocol(下次渲染重建)。
    fn drain_ready_covers(&mut self) {
        for (url, image) in self.cover_fetcher.drain_ready() {
            self.state.cover_pending.remove(&url);
            self.state.cover_cache.insert(url.clone(), image);
            self.state.cover_protocols.borrow_mut().remove(&url);
        }
    }

    /// 把 client.pull_pcm 拿到的样本喂给 fft computer。in-proc 和 connect 走同一路径。
    fn update_spectrum(&mut self) {
        const POP_CHUNK: usize = 2048;
        let (samples, sample_rate) = self.client.pull_pcm(POP_CHUNK);
        if !samples.is_empty() {
            self.state.fft.push(&samples);
        }
        let target_bars = self.state.spectrum.target_bars.get();
        let bars = self.state.fft.compute(sample_rate, target_bars);
        self.state.spectrum.tick(
            self.state.playback.playing,
            self.state.playback.volume_pct,
            bars.as_deref(),
        );
    }

    /// 把 server 端积攒的 task events 拉过来 apply 到 [`AppState`]。
    fn drain_task_events(&mut self) {
        let events = self.client.drain_task_events();
        for ev in &events {
            // server 已 filter PlayUrlReady / LyricsReady,这里只剩 playlists/tracks/liked。
            self.state.apply(ev);
            match ev {
                TaskEvent::PlaylistsFetched { .. }
                | TaskEvent::PlaylistTracksFetched { .. }
                | TaskEvent::LikedSongIdsFetched { .. }
                | TaskEvent::PlayUrlReady { .. }
                | TaskEvent::LyricsReady { .. } => {}
            }
        }
    }

    /// 处理一个 crossterm 事件;目前只关心 KeyEvent 的按下边沿。
    fn handle_event(&mut self, ev: &Event) {
        if let Event::Key(key) = ev
            && key.kind == KeyEventKind::Press
        {
            self.handle_key(key);
        }
    }

    /// 顶层按键分发:Ctrl-C 永远退出,然后按当前 modal/focus/view 路由到具体 handler。
    fn handle_key(&mut self, key: &KeyEvent) {
        // Ctrl-C 强制退出(skip confirm)。
        if matches!(
            (key.modifiers, key.code),
            (KeyModifiers::CONTROL, KeyCode::Char('c'))
        ) {
            self.should_quit = true;
            return;
        }

        if self.state.search_mode {
            self.handle_search_key(key);
            return;
        }

        if self.state.confirm_open {
            self.handle_confirm_key(key);
            return;
        }

        if key.code == KeyCode::Tab {
            self.toggle_queue();
            return;
        }

        if key.code == KeyCode::Char('q') {
            if self.state.queue_open {
                self.close_queue();
            } else {
                self.state.confirm_open = true;
            }
            return;
        }

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

        if key.code == KeyCode::Char('/') {
            self.state.search_mode = true;
            self.state.search_q.clear();
            return;
        }

        if self.handle_playback_key(key) {
            return;
        }

        match self.state.view {
            View::Playlists => self.handle_playlists_key(key),
            View::Library => self.handle_library_key(key),
        }
    }

    /// 退出确认弹窗的按键:y/Y/Enter 退出,n/N/Esc 关闭弹窗。
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

    /// 搜索词每次变化后,把当前 view 的 sel 拉回 0。
    fn reset_sel_for_search(&mut self) {
        match self.state.view {
            View::Playlists => self.state.sel_playlist = 0,
            View::Library => self.state.sel_track = 0,
        }
    }

    /// 搜索输入态按键:Esc 退出 + 清词,Enter 退出保留词,Backspace/字符更新词并复位 sel。
    fn handle_search_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.state.search_mode = false;
                self.state.search_q.clear();
            }
            KeyCode::Enter => {
                self.state.search_mode = false;
            }
            KeyCode::Backspace => {
                self.state.search_q.pop();
                self.reset_sel_for_search();
                self.state.last_sel_change = Instant::now();
            }
            KeyCode::Char(c) => {
                self.state.search_q.push(c);
                self.reset_sel_for_search();
                self.state.last_sel_change = Instant::now();
            }
            _ => {}
        }
    }

    /// 切换 queue overlay 的开关 + 同步焦点。
    fn toggle_queue(&mut self) {
        self.state.queue_open = !self.state.queue_open;
        self.state.focus = if self.state.queue_open {
            Focus::Queue
        } else {
            Focus::Left
        };
    }

    /// 强制关闭 queue overlay 并把焦点收回左侧。
    fn close_queue(&mut self) {
        self.state.queue_open = false;
        self.state.focus = Focus::Left;
    }

    /// queue 焦点下的按键:vim 风格上下移动 + Enter 选择当前行播放。
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
                    self.client.play_song(s);
                }
            }
            _ => {}
        }
    }

    /// 与播放控制相关的全局按键(空格/m/+/-/方向/p/n)。返回 true 表示按键已被消化。
    fn handle_playback_key(&mut self, key: &KeyEvent) -> bool {
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
            KeyCode::Char('m') => self.client.cycle_play_mode(),
            KeyCode::Char('+') | KeyCode::Char('=') => self.nudge_volume(VOLUME_STEP),
            KeyCode::Char('-') | KeyCode::Char('_') => self.nudge_volume(-VOLUME_STEP),
            KeyCode::Left => self.seek_relative(-SEEK_STEP_S),
            KeyCode::Right => self.seek_relative(SEEK_STEP_S),
            KeyCode::Char('p') => self.client.prev_or_restart(),
            KeyCode::Char('n') => self.client.next_song(),
            _ => return false,
        }
        true
    }

    /// 空格键:有当前曲目时在 pause/resume 间切换;没歌时无动作。
    fn toggle_play_pause(&mut self) {
        if self.state.playback.track.is_none() {
            return;
        }
        if self.state.playback.playing {
            self.client.pause();
        } else {
            self.client.resume();
        }
    }

    /// 在当前音量上加/减 `delta`,clamp 到 0..=100,本地立即更新避免 UI 滞后。
    fn nudge_volume(&mut self, delta: i16) {
        let cur = i16::from(self.state.playback.volume_pct);
        let new = cur.saturating_add(delta).clamp(0, 100);
        let pct = u8::try_from(new).unwrap_or(self.state.playback.volume_pct);
        self.client.set_volume(pct);
        self.state.playback.volume_pct = pct;
    }

    /// 相对当前位置跳 `delta_s` 秒,clamp 到 [0, duration]。
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
        self.client.seek(new_u);
    }

    /// Playlists view 的按键:vim 风格上下移动 + Enter/l 进入选中歌单的 Library。
    fn handle_playlists_key(&mut self, key: &KeyEvent) {
        self.state.last_sel_change = Instant::now();
        if matches!(key.code, KeyCode::Esc) && !self.state.search_q.is_empty() {
            self.state.search_q.clear();
            self.state.sel_playlist = 0;
            return;
        }
        let max = self.state.filtered_playlists().len().saturating_sub(1);
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
                self.state.sel_playlist = max;
            }
            KeyCode::Char('l') | KeyCode::Enter => {
                if let Some(target_id) = self
                    .state
                    .filtered_playlists()
                    .get(self.state.sel_playlist)
                    .map(|p| p.data.id.clone())
                {
                    self.state.search_q.clear();
                    if let Some(raw_idx) = self
                        .state
                        .playlists
                        .iter()
                        .position(|p| p.data.id == target_id)
                    {
                        self.state.sel_playlist = raw_idx;
                    }
                }
                self.state.view = View::Library;
                self.state.sel_track = 0;
            }
            _ => {}
        }
    }

    /// Library view 的按键:vim 风格上下 + h/Esc/Backspace 回 Playlists + Enter 设 queue 并播放。
    fn handle_library_key(&mut self, key: &KeyEvent) {
        self.state.last_sel_change = Instant::now();
        if matches!(key.code, KeyCode::Esc) && !self.state.search_q.is_empty() {
            self.state.search_q.clear();
            self.state.sel_track = 0;
            return;
        }
        let max = self.state.filtered_tracks().len().saturating_sub(1);
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
                self.state.sel_track = max;
            }
            KeyCode::Char('h') | KeyCode::Esc | KeyCode::Backspace => {
                self.state.search_q.clear();
                self.state.view = View::Playlists;
            }
            KeyCode::Enter => {
                let filtered = self.state.filtered_tracks();
                let Some(song) = filtered.get(self.state.sel_track).map(|sv| sv.data.clone())
                else {
                    return;
                };
                let new_queue: Vec<Song> = self
                    .state
                    .current_tracks()
                    .into_iter()
                    .map(|sv| sv.data)
                    .collect();
                // Server 端按 PlayMode 决定要不要洗牌;client 只发原始 queue + target。
                self.client.set_queue(new_queue, song.id.clone());
                self.client.play_song(song);
            }
            _ => {}
        }
    }
}
