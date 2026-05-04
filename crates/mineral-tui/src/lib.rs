//! Terminal UI client for Mineral.

#[cfg(windows)]
compile_error!("Windows 暂不支持");

mod app;
mod color;
mod components;
mod cover;
mod layout;
mod lrc;
mod playback;
mod prefetch;
mod state;
mod theme;
mod tui;
mod view;
mod view_model;
mod yrc;

use std::sync::Arc;

use color_eyre::eyre::eyre;
use mineral_channel_core::MusicChannel;
use mineral_server::Server;
use ratatui_image::picker::Picker;

use app::App;
use tui::Tui;

/// TUI 退出时是否一并关掉 server。
///
/// 单进程下 TUI 退 = 进程退 = server 也走,这条 const 实际无可观察差别;
/// 留作「真正拆 daemon 之后」的开关——届时设 `false` 表示「仅关 client,
/// 后台 server 继续播」(对齐 mpd / playerctl 那种行为)。当前写死 `true`,
/// 与历史 UX 一致。
const KILL_SERVER_ON_TUI_EXIT: bool = true;

/// 启动 TUI。
///
/// # Params:
///   - `channels`: 已构造好的所有音乐源。TUI 平等对待,扔给 server 后
///     scheduler 会逐个 channel 拉 `my_playlists`。空 vec 也合法(纯 UI 演示)。
///
/// # Return:
///   主循环正常退出返回 `Ok(())`;终端 raw mode / 渲染失败返回 `Err`。
pub async fn run(channels: Vec<Arc<dyn MusicChannel>>) -> color_eyre::Result<()> {
    let mut server = Server::spawn(channels)?;
    let spectrum_tap = server
        .take_spectrum_tap()
        .ok_or_else(|| eyre!("Server::take_spectrum_tap 已被取走;TUI 期望第一次取得 tap"))?;
    let client = server.client();
    let cover_fetcher = cover::CoverFetcher::spawn()?;

    let mut tui = Tui::new()?;
    tui.enter()?;
    // Picker::from_query_stdio 必须在进 alternate screen 之后、读 events 之前调,
    // 因为它会临时往 stdio 写探测 escape 序列读响应。失败 fallback 到 8x16 fixed
    // font 用 halfblocks 渲染,不阻塞启动。
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((8, 16)));
    let mut app = App::new(client, spectrum_tap, cover_fetcher, picker);
    let result = app.run(&mut tui);
    tui.exit()?;

    if KILL_SERVER_ON_TUI_EXIT {
        server.shutdown();
    }
    // false 分支:在真正拆双进程之前不可达。单进程下 server drop 不 drop 都
    // 没意义——进程退出时 OS 会回收一切。等 daemon 化之后再实现 detach 逻辑。
    result
}
