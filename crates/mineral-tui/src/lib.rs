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
mod remote;
mod state;
mod theme;
mod tui;
mod view;
mod view_model;
mod yrc;

use std::sync::Arc;

use color_eyre::eyre::eyre;
use mineral_audio::SpectrumTap;
use mineral_channel_core::MusicChannel;
use mineral_server::{Client, Server};
use ratatui_image::picker::Picker;

use app::App;
use cover::CoverFetcher;
use remote::RemoteClient;
use tui::Tui;

/// 启动 TUI。
///
/// 两种模式:
/// - **in-proc** (`connect = false`):TUI 自己 `Server::spawn`,持有 audio engine /
///   scheduler / 等;关 TUI = 进程退 = server 跟着退。等价历史行为。
/// - **connect** (`connect = true`):TUI 不起 server,只 [`RemoteClient::connect`] 到
///   `mineral serve` 起的 daemon。关 TUI **不杀** daemon,音乐继续播。
///
/// # Params:
///   - `channels`: in-proc 模式下,已构造好的所有音乐源(空 vec 也合法,纯 UI 演示);
///     connect 模式下忽略(channels 由 daemon 持有)。
///   - `connect`: true → 连 daemon;false → 自起 server。
pub async fn run(channels: Vec<Arc<dyn MusicChannel>>, connect: bool) -> color_eyre::Result<()> {
    let cover_fetcher = CoverFetcher::spawn()?;

    if connect {
        let socket = mineral_paths::runtime_dir()?.join("mineral.sock");
        let client = RemoteClient::connect(&socket).await?;
        run_app(Arc::new(client), None, cover_fetcher)
    } else {
        let mut server = Server::spawn(channels)?;
        let spectrum_tap = server
            .take_spectrum_tap()
            .ok_or_else(|| eyre!("Server::take_spectrum_tap 已被取走;TUI 期望第一次取得 tap"))?;
        let client = server.client();
        let result = run_app(Arc::new(client), Some(spectrum_tap), cover_fetcher);
        // in-proc 模式:进程退 = server 跟着 drop,无显式 shutdown 也行。
        // server 保留到这里只为让 audio engine 跑完 TUI 主循环。
        let _ = server;
        result
    }
}

fn run_app(
    client: Arc<dyn Client>,
    spectrum_tap: Option<SpectrumTap>,
    cover_fetcher: CoverFetcher,
) -> color_eyre::Result<()> {
    let mut tui = Tui::new()?;
    tui.enter()?;
    // Picker::from_query_stdio 必须在进 alternate screen 之后、读 events 之前调,
    // 因为它会临时往 stdio 写探测 escape 序列读响应。失败 fallback 到 8x16 fixed
    // font 用 halfblocks 渲染,不阻塞启动。
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((8, 16)));
    let mut app = App::new(client, spectrum_tap, cover_fetcher, picker);
    let result = app.run(&mut tui);
    tui.exit()?;
    result
}
