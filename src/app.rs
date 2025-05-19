use ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
    text::Line,
    widgets::{Block, Borders},
};
use std::io::{self};

pub struct App {
    should_exit: bool,
    text: String,
}

impl App {
    pub fn default() -> Self {
        App {
            should_exit: false,
            text: String::from("按q退出程序"),
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        loop {
            if self.should_exit {
                break;
            }

            terminal.draw(|frame| self.draw(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        let para = Block::default()
            .title(Line::from(self.text.as_str()).alignment(ratatui::layout::Alignment::Center))
            .borders(Borders::ALL);

        frame.render_widget(para, frame.area());
    }

    fn handle_events(&mut self) -> io::Result<()> {
        match event::read()? {
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                self.handle_key_event(key_event)
            }
            _ => {}
        };
        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Char('q') => self.should_exit = true,
            _ => {}
        }
    }
}
