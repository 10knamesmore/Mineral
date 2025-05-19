use app::App;
use std::io::Result;

mod app;
mod ui;

fn main() -> Result<()> {
    let mut terminal = ratatui::init();
    let res = App::default().run(&mut terminal);
    ratatui::restore();

    res
}
