//! Terminal UI client for Mineral.

#[cfg(windows)]
compile_error!("Windows 暂不支持");

mod app;
mod components;
mod layout;
mod playback;
mod state;
mod theme;
mod tui;
mod view;
mod view_model;

use app::App;
use tui::Tui;

/// Run the Mineral TUI client.
pub fn run() -> color_eyre::Result<()> {
    let mut tui = Tui::new()?;
    tui.enter()?;
    let result = App::new().run(&mut tui);
    tui.exit()?;
    result
}
