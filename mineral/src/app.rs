//! 顶层 [`App`] 状态与同步主事件循环。

use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::state::{AppState, Focus, View};
use crate::theme::Theme;
use crate::tui::Tui;
use crate::view::draw;

/// 应用顶层状态。
pub struct App {
    /// 是否退出主循环。
    pub should_quit: bool,
    /// 当前主题。
    pub theme: Theme,
    /// 业务状态(视图、选中、playback、mock 数据等)。
    pub state: AppState,
    /// 上一次 tick 时间。
    pub last_tick: Instant,
}

impl App {
    /// 构造默认 App(Mocha mauve 主题 + mock 数据)。
    pub fn new() -> Self {
        Self {
            should_quit: false,
            theme: Theme::default(),
            state: AppState::new(),
            last_tick: Instant::now(),
        }
    }

    /// 同步主事件循环:绘制 → 等事件 / tick → 处理 → 重绘。
    pub fn run(&mut self, tui: &mut Tui) -> color_eyre::Result<()> {
        let tick_rate = Duration::from_millis(250);

        while !self.should_quit {
            tui.draw(|f| draw(f, self))?;

            let timeout = tick_rate.saturating_sub(self.last_tick.elapsed());
            if event::poll(timeout)? {
                self.handle_event(&event::read()?);
            }
            if self.last_tick.elapsed() >= tick_rate {
                let dt = self.last_tick.elapsed();
                self.state.playback.tick(dt);
                self.state.spectrum.tick(self.state.playback.playing);
                self.last_tick = Instant::now();
            }
        }
        Ok(())
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

        // 全局 playback 键(Space / m / s / +- / ←→ / p / n)。
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
                    self.state.current = Some(s.clone());
                    self.state.playback.track = Some(s);
                    self.state.playback.position_ms = 0;
                    self.state.playback.playing = true;
                }
            }
            _ => {}
        }
    }

    fn handle_playback_key(&mut self, key: &KeyEvent) -> bool {
        let pb = &mut self.state.playback;
        match key.code {
            KeyCode::Char(' ') => pb.play_pause(),
            KeyCode::Char('m') => pb.mode = pb.mode.cycle(),
            KeyCode::Char('s') => pb.sort = pb.sort.cycle(),
            KeyCode::Char('+') | KeyCode::Char('=') => pb.nudge_volume(5),
            KeyCode::Char('-') | KeyCode::Char('_') => pb.nudge_volume(-5),
            KeyCode::Left => pb.seek(-5),
            KeyCode::Right => pb.seek(5),
            KeyCode::Char('p') | KeyCode::Char('n') => {
                // stage 4 mock — 没有 queue 概念,留空
            }
            _ => return false,
        }
        true
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
                    self.state.current = Some(s.clone());
                    self.state.playback.track = Some(s);
                    self.state.playback.position_ms = 0;
                    self.state.playback.playing = true;
                    self.state.queue = tracks.into_iter().map(|sv| sv.data).collect();
                    self.state.queue_sel = self.state.sel_track;
                }
            }
            _ => {}
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
