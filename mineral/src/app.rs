//! 顶层 [`App`] 状态与同步主事件循环。

use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::cmd::{self, CmdEffect, CmdMode};
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
                self.expire_hint();
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

        // 最高 UI 优先级:cmd 模式吞掉所有键。
        if self.state.cmd_mode.is_some() {
            self.handle_cmd_key(key);
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

        // / 进入 search 模式,: 进入 command 模式。
        if key.code == KeyCode::Char('/') {
            self.state.cmd_mode = Some(CmdMode::Search);
            self.state.cmd_buffer.clear();
            self.state.search_q.clear();
            return;
        }
        if key.code == KeyCode::Char(':') {
            self.state.cmd_mode = Some(CmdMode::Command);
            self.state.cmd_buffer.clear();
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

    fn handle_cmd_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.state.cmd_mode = None;
                self.state.cmd_buffer.clear();
                self.state.search_q.clear();
            }
            KeyCode::Enter => {
                let mode = self.state.cmd_mode;
                let buf = std::mem::take(&mut self.state.cmd_buffer);
                self.state.cmd_mode = None;
                if mode == Some(CmdMode::Command) {
                    for eff in cmd::parse(&buf) {
                        self.apply_effect(eff);
                    }
                }
                // search 模式回车提交后保留 search_q 持续过滤(esc 才清)。
            }
            KeyCode::Backspace => {
                self.state.cmd_buffer.pop();
                if self.state.cmd_mode == Some(CmdMode::Search) {
                    self.state.search_q.clone_from(&self.state.cmd_buffer);
                    self.state.sel_playlist = 0;
                    self.state.sel_track = 0;
                }
            }
            KeyCode::Char(c) => {
                self.state.cmd_buffer.push(c);
                if self.state.cmd_mode == Some(CmdMode::Search) {
                    self.state.search_q.clone_from(&self.state.cmd_buffer);
                    self.state.sel_playlist = 0;
                    self.state.sel_track = 0;
                }
            }
            _ => {}
        }
    }

    fn apply_effect(&mut self, eff: CmdEffect) {
        match eff {
            CmdEffect::Quit => self.should_quit = true,
            CmdEffect::SetMode(m) => {
                self.state.playback.mode = m;
                self.set_hint(format!("mode → {}", m.label()));
            }
            CmdEffect::SetSort(s) => {
                self.state.playback.sort = s;
                self.set_hint(format!("sort → {}", s.label()));
            }
            CmdEffect::SetAccent(name) => {
                self.theme = self.theme.with_accent_pair(&name);
                self.set_hint(format!("accent → {name}"));
            }
            CmdEffect::SetTheme(name) => {
                self.set_hint(format!("theme '{name}' not yet supported"));
            }
            CmdEffect::Play(n) => {
                let label = n.map_or_else(|| "current".to_owned(), |i| format!("#{i}"));
                self.set_hint(format!(":play {label} not yet supported"));
            }
            CmdEffect::Hint(msg) => self.set_hint(msg),
        }
    }

    fn set_hint(&mut self, msg: String) {
        let deadline = Instant::now() + Duration::from_secs(3);
        self.state.hint = Some((msg, deadline));
    }

    fn expire_hint(&mut self) {
        if let Some((_, deadline)) = self.state.hint.as_ref() {
            if Instant::now() > *deadline {
                self.state.hint = None;
            }
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
