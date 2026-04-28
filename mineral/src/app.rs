//! 顶层 [`App`] 状态与同步主事件循环。

use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::theme::Theme;
use crate::tui::Tui;
use crate::view::draw;

/// 应用顶层状态。后续阶段会逐步把 sidebar / playback / cmd 状态加进来。
pub struct App {
    /// 是否退出主循环。
    pub should_quit: bool,
    /// 当前主题。
    pub theme: Theme,
    /// 上一次 tick 时间。
    pub last_tick: Instant,
}

impl App {
    /// 构造默认 App(Mocha mauve 主题)。
    pub fn new() -> Self {
        Self {
            should_quit: false,
            theme: Theme::default(),
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
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('c')) | (_, KeyCode::Char('q')) => {
                self.should_quit = true;
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
