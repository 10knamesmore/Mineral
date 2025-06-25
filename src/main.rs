use app::App;
use std::{io::Result, path::Path};

use crate::app::{data_generator::test_struct_app, logger};

mod api;
mod app;
mod event_handler;
mod state;
mod ui;
mod util;

#[tokio::main]
async fn main() -> Result<()> {
    logger::init(Path::new("logs").join("outputs.log")).unwrap();

    let mut terminal = ratatui::init();
    let res = test_struct_app().run(&mut terminal).await;
    ratatui::restore();

    res
}
