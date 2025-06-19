use app::{App, data_generator};
use std::io::Result;

mod app;
mod event_handler;
mod state;
mod ui;
mod util;

#[tokio::main]
async fn main() -> Result<()> {
    let mut terminal = ratatui::init();
    let res = data_generator::test_struct_app().run(&mut terminal).await;
    ratatui::restore();

    res
}
