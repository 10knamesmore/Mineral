use app::App;
use std::io::Result;

use crate::app::data_generator::test_struct_app;

mod api;
mod app;
mod event_handler;
mod state;
mod ui;
mod util;

#[tokio::main]
async fn main() -> Result<()> {
    let mut terminal = ratatui::init();
    let res = test_struct_app().run(&mut terminal).await;
    ratatui::restore();

    res
}
