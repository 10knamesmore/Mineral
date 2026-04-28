//! Mineral TUI 音乐播放器入口。
//!
//! 流程:
//! 1. 安装 `color_eyre` 美化错误输出
//! 2. 创建 [`Tui`] guard,进入 raw mode + alternate screen
//! 3. 运行 [`App`] 主循环
//! 4. 退出 TUI(显式调用 + Drop 兜底)

#[cfg(windows)]
compile_error!("Windows 暂不支持");

mod app;
mod cmd;
mod components;
mod layout;
mod playback;
mod state;
mod theme;
mod tui;
mod view;
mod view_model;

use crate::app::App;
use crate::tui::Tui;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let mut tui = Tui::new()?;
    tui.enter()?;
    let result = App::new().run(&mut tui);
    tui.exit()?;
    result
}
