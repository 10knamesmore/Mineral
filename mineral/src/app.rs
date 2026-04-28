//! 顶层 [`App`] 状态与同步主事件循环。

use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::state::{AppState, View};
use crate::theme::Theme;
use crate::tui::Tui;
use crate::view::draw;

/// 应用顶层状态。
pub struct App {
    /// 是否退出主循环。
    pub should_quit: bool,
    /// 当前主题。
    pub theme: Theme,
    /// 业务状态(视图、选中、mock 数据等)。
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
        if matches!(
            (key.modifiers, key.code),
            (KeyModifiers::CONTROL, KeyCode::Char('c')) | (_, KeyCode::Char('q'))
        ) {
            self.should_quit = true;
            return;
        }
        match self.state.view {
            View::Playlists => self.handle_playlists_key(key),
            View::Library => self.handle_library_key(key),
        }
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
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
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
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Esc | KeyCode::Backspace => {
                self.state.view = View::Playlists;
            }
            KeyCode::Enter => {
                if let Some(s) = self
                    .state
                    .current_tracks()
                    .get(self.state.sel_track)
                    .map(|sv| sv.data.clone())
                {
                    self.state.current = Some(s);
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
