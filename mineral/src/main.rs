use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{DefaultTerminal, Frame};

#[cfg(windows)]
compile_error!("Windows暂不支持");

fn main() -> Result<()> {
    let mut terminal = ratatui::init();
    let res = run(&mut terminal);
    ratatui::restore();
    res
}

fn run(terminal: &mut DefaultTerminal) -> Result<()> {
    loop {
        terminal.draw(draw)?;
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
                return Ok(());
            }
        }
    }
}

fn draw(frame: &mut Frame) {
    frame.render_widget("Mineral - press q to quit", frame.area());
}
