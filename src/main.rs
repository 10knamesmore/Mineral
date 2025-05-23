use app::App;
use std::io::Result;

mod app;
mod event_handler;
mod state;
mod ui;
mod util;

fn main() -> Result<()> {
    let mut terminal = ratatui::init();
    // let res = App::default().run(&mut terminal);
    let res = app::test_data().test_run(&mut terminal);
    ratatui::restore();

    res
}
