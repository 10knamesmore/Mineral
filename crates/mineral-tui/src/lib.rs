//! Terminal UI client for Mineral.

#[cfg(windows)]
compile_error!("Windows 暂不支持");

mod app;
mod applog;
mod components;
mod layout;
mod loader;
mod playback;
mod state;
mod theme;
mod tui;
mod view;
mod view_model;

use std::sync::Arc;

use mineral_channel_core::MusicChannel;

use app::App;
use loader::spawn_initial_load;
use tui::Tui;

/// 启动 TUI。
///
/// # Params:
///   - `channels`: 已构造好的所有音乐源。TUI 平等对待,逐个调用
///     `MusicChannel::my_playlists` 拉取贡献。空 vec 也合法(纯 UI 演示)。
///
/// # Return:
///   主循环正常退出返回 `Ok(())`;终端 raw mode / 渲染失败返回 `Err`。
pub async fn run(channels: Vec<Arc<dyn MusicChannel>>) -> color_eyre::Result<()> {
    let mut app = App::new();
    app.attach_loader(spawn_initial_load(channels));
    let mut tui = Tui::new()?;
    tui.enter()?;
    let result = app.run(&mut tui);
    tui.exit()?;
    result
}
