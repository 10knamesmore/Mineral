use app::App;

mod api;
mod app;
mod event_handler;
mod state;
mod ui;
mod util;

#[cfg(windows)]
compile_error!("Windows暂不支持");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = mineral_log::init().unwrap();

    let mut terminal = ratatui::init();
    let res = App::init()?.run(&mut terminal).await;
    ratatui::restore();

    res
}
