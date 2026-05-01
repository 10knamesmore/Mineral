//! 顶层 [`App`] 状态与同步主事件循环。

use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use mineral_audio::AudioHandle;
use mineral_model::Song;
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

/// seek 步长(秒);`←`/`→` 一次。
const SEEK_STEP_S: i64 = 5;

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
}

impl App {
    /// 构造 App。
    ///
    /// # Params:
    ///   - `scheduler`: 已构造的调度器,UI 持有共享引用
    ///   - `audio`: 已启动的音频引擎句柄
    pub fn new(scheduler: Scheduler, audio: AudioHandle) -> Self {
        Self {
            should_quit: false,
            theme: Theme::default(),
            state: AppState::empty(),
            last_tick: Instant::now(),
            scheduler,
            audio,
        }
    }

    /// 同步主事件循环:绘制 → 等事件 / tick → 处理 → 重绘。
    pub fn run(&mut self, tui: &mut Tui) -> color_eyre::Result<()> {
        let tick_rate = Duration::from_millis(250);

        while !self.should_quit {
            self.drain_task_events();
            tui.draw(|f| draw(f, self))?;

            let timeout = tick_rate.saturating_sub(self.last_tick.elapsed());
            if event::poll(timeout)? {
                self.handle_event(&event::read()?);
            }
            if self.last_tick.elapsed() >= tick_rate {
                self.state
                    .playback
                    .apply_audio_snapshot(self.audio.snapshot());
                self.state.spectrum.tick(self.state.playback.playing);
                self.last_tick = Instant::now();
            }
        }
        Ok(())
    }

    fn drain_task_events(&mut self) {
        let events = self.scheduler.drain_events();
        for ev in &events {
            self.state.apply(ev);
            match ev {
                TaskEvent::PlaylistsFetched { playlists, .. } => {
                    self.fanout_playlist_tracks(playlists);
                }
                TaskEvent::PlayUrlReady { play_url, .. } => {
                    self.audio.play(play_url.url.clone());
                }
                TaskEvent::PlaylistTracksFetched { .. } => {}
            }
        }
    }

    /// Enter 一首歌:杀掉旧 PlayPrep + audio.stop,然后提交新 PlayPrep。
    /// 收到 `PlayUrlReady` 后才真正出声。
    fn submit_play_song(&mut self, song: &Song) {
        self.scheduler.cancel_where(|k| {
            matches!(k, TaskKind::ChannelFetch(ChannelFetchKind::SongUrl { .. }))
        });
        self.audio.stop();
        self.state.current = Some(song.clone());
        self.state.playback.track = Some(song.clone());
        self.state.playback.position_ms = 0;
        self.scheduler.submit(
            TaskKind::ChannelFetch(ChannelFetchKind::SongUrl {
                source: song.source,
                song_id: song.id.clone(),
            }),
            Priority::User,
        );
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

        // queue focused 时,Esc 关闭 queue;其他键走 queue handler。
        if self.state.focus == Focus::Queue {
            if key.code == KeyCode::Esc {
                self.close_queue();
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
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                let max = len.saturating_sub(1);
                self.state.queue_sel = self.state.queue_sel.saturating_add(1).min(max);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.state.queue_sel = self.state.queue_sel.saturating_sub(1);
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
        match key.code {
            KeyCode::Char(' ') => self.toggle_play_pause(),
            KeyCode::Char('m') => self.state.playback.mode = self.state.playback.mode.cycle(),
            KeyCode::Char('+') | KeyCode::Char('=') => self.nudge_volume(VOLUME_STEP),
            KeyCode::Char('-') | KeyCode::Char('_') => self.nudge_volume(-VOLUME_STEP),
            KeyCode::Left => self.seek_relative(-SEEK_STEP_S),
            KeyCode::Right => self.seek_relative(SEEK_STEP_S),
            KeyCode::Char('p') | KeyCode::Char('n') => {
                // 单曲模式下无 prev/next 概念;auto-next lane 那一期再说。
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
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                let max = self.state.playlists.len().saturating_sub(1);
                self.state.sel_playlist = self.state.sel_playlist.saturating_add(1).min(max);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.state.sel_playlist = self.state.sel_playlist.saturating_sub(1);
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
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                let max = len.saturating_sub(1);
                self.state.sel_track = self.state.sel_track.saturating_add(1).min(max);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.state.sel_track = self.state.sel_track.saturating_sub(1);
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
                    self.state.queue = tracks.into_iter().map(|sv| sv.data).collect();
                    self.state.queue_sel = self.state.sel_track;
                    self.submit_play_song(&s);
                }
            }
            _ => {}
        }
    }
}
