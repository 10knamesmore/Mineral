use app::App;
use std::path::Path;

use crate::app::logger;

mod api;
mod app;
mod event_handler;
mod state;
mod ui;
mod util;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    logger::init(Path::new("logs").join("outputs.log")).unwrap();

    let mut terminal = ratatui::init();
    let res = App::init()?.run(&mut terminal).await;
    ratatui::restore();

    res
}
