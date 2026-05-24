//! Terminal UI client for Mineral.

#[cfg(windows)]
compile_error!("Windows 暂不支持");

mod app;
mod color;
mod components;
mod cover;
mod daemon;
mod layout;
mod playback;
mod prefetch;
mod remote;
mod signal;
mod state;
#[cfg(test)]
mod test_support;
mod theme;
mod tui;
mod view;
mod view_model;

use std::sync::Arc;

use mineral_channel_core::MusicChannel;
use mineral_server::{Client, Server};
use ratatui_image::picker::Picker;

use app::App;
use cover::CoverFetcher;
use remote::RemoteClient;
use tui::Tui;

/// TUI 的启动模式。决定 server 的来源与生命周期。
pub enum Launch {
    /// 默认:优先 attach 已有 daemon;没有则 spawn 一个独立 `mineral serve`
    /// 子进程再 attach。client 退出时是否连带 kill 掉自己 spawn 的 daemon,由
    /// daemon 模块内的 const 旋钮决定(当前 = 一起退)。
    Auto,

    /// 强制连已有 daemon(连不上即报错,**不** spawn)。
    Connect,

    /// in-proc:TUI 自己 `Server::spawn`,同进程持有 audio engine / scheduler /
    /// PlayerCore;关 TUI = 进程退 = server 跟着退。调试 / 离线开发用。
    InProc,
}

/// 启动 TUI。
///
/// 三种模式见 [`Launch`]。所有模式下 spectrum 都走 `client.pull_pcm` —— PCM 中继
/// 统一在 server 内部,in-proc 也通过同一接口拉(零拷贝优势让位于接口统一)。
///
/// # Params:
///   - `channels`: 仅 [`Launch::InProc`] 用到(已构造好的全部音乐源,空 vec 也合法);
///     `Auto` / `Connect` 下忽略 —— channels 由独立 daemon 进程自己持有。
///   - `launch`: 启动模式。
pub async fn run(channels: Vec<Arc<dyn MusicChannel>>, launch: Launch) -> color_eyre::Result<()> {
    // 封面 fetcher 起不来(isahc / TLS / 证书)不该拖垮整个 TUI —— 降级到禁用态空跑,
    // 与音频无设备降级 null 模式同理。封面不显示,其余功能照常。
    let cover_fetcher = CoverFetcher::spawn().unwrap_or_else(|e| {
        mineral_log::warn!(
            error = mineral_log::chain(&e),
            "cover fetcher 起步失败,封面禁用"
        );
        CoverFetcher::disabled()
    });

    match launch {
        Launch::Auto => {
            let socket = mineral_paths::socket_path()?;
            let (client, handle) = daemon::ensure(&socket).await?;
            let result = run_app(Arc::new(client), cover_fetcher);
            // client 退出:仅当本次亲手 spawn 了 daemon 才按旋钮收尾;attach 已有的
            // (handle 为 None)留着不动。
            if let Some(handle) = handle {
                handle.shutdown_if_owned();
            }
            result
        }
        Launch::Connect => {
            let socket = mineral_paths::socket_path()?;
            let client = RemoteClient::connect(&socket).await?;
            run_app(Arc::new(client), cover_fetcher)
        }
        Launch::InProc => {
            // in-proc 调试:走 Auto,本机有声卡就真出声,没有则降级 null。
            let server = Server::spawn(channels, mineral_server::AudioMode::Auto)?;
            // in-proc 也接系统媒体服务(MPRIS),单跑 TUI 时桌面控件 / 媒体键照样联动;
            // 无 D-Bus session 时降级。进程退 = server drop = MPRIS 注销。
            if let Err(e) = server.start_media_service() {
                mineral_log::warn!(target: "media", error = mineral_log::chain(&e), "system media service unavailable");
            }
            let client = server.client();
            let result = run_app(Arc::new(client), cover_fetcher);
            // in-proc 模式:进程退 = server 跟着 drop,无显式 shutdown 也行。
            let _ = server;
            result
        }
    }
}

/// 拿到一个 client(in-proc 或 remote 都行),进 alternate screen,起 ratatui-image picker,
/// 跑 [`App::run`] 直到退出,最后还原终端。
fn run_app(client: Arc<dyn Client>, cover_fetcher: CoverFetcher) -> color_eyre::Result<()> {
    let mut tui = Tui::new()?;
    tui.enter()?;
    // Picker::from_query_stdio 必须在进 alternate screen 之后、读 events 之前调,
    // 因为它会临时往 stdio 写探测 escape 序列读响应。失败 fallback 到 8x16 fixed
    // font 用 halfblocks 渲染,不阻塞启动。
    let picker = Picker::from_query_stdio().unwrap_or_else(|_| Picker::from_fontsize((8, 16)));
    let mut app = App::new(client, cover_fetcher, picker);
    let result = app.run(&mut tui);
    tui.exit()?;
    result
}
