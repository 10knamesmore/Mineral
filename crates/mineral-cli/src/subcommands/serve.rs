//! `mineral serve` — 起 daemon,bind unix socket,跑 `Server::serve` accept loop。
//!
//! 调用方(main.rs)负责 build channels 后传进来。本模块负责:
//! 1. 解析 socket 路径
//! 2. stale socket 检测(已活 daemon → bail;残留 socket 文件 → 删)
//! 3. bind + Server::spawn + serve

use std::sync::Arc;

use color_eyre::eyre::{WrapErr, bail};
use mineral_channel_core::MusicChannel;
use mineral_server::Server;
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};

/// daemon 入口。需要 caller 已 `tokio::Runtime::block_on` 或在 async ctx 中调。
///
/// accept loop 与关闭信号(SIGINT / SIGTERM)竞争:收到信号时主动停掉 server
/// (走 [`Server::shutdown`] 的 Drop 链)并 unlink socket 文件,避免残留 stale
/// socket 留给下次启动清理。
pub async fn run(channels: Vec<Arc<dyn MusicChannel>>) -> color_eyre::Result<()> {
    let socket_path = socket_path()?;
    prepare_socket(&socket_path).await?;
    let listener = UnixListener::bind(&socket_path)
        .wrap_err_with(|| format!("bind unix socket {}", socket_path.display()))?;
    println!("mineral daemon listening on {}", socket_path.display());

    let server = Server::spawn(channels)?;
    // 接入系统媒体服务(MPRIS)。无 D-Bus session 等失败时降级:daemon 照常跑。
    if let Err(e) = server.start_media_service() {
        mineral_log::warn!(target: "media", "system media service unavailable: {e}");
    }
    let outcome = tokio::select! {
        result = server.serve(listener) => result,
        result = shutdown_signal() => result.map(|()| {
            mineral_log::info!(target: "ipc", "shutdown signal received, stopping daemon");
        }),
    };

    // graceful 收尾:停 server(Drop 链停 audio engine / scheduler)+ 清 socket。
    server.shutdown();
    if let Err(e) = std::fs::remove_file(&socket_path) {
        mineral_log::warn!(
            target: "ipc",
            "remove socket {} on shutdown: {e}",
            socket_path.display()
        );
    }
    outcome
}

/// 等待第一个到达的关闭信号(SIGINT 或 SIGTERM)。
///
/// # Return:
///   信号 handler 安装成功且收到信号 → `Ok(())`;安装失败 → `Err`。
async fn shutdown_signal() -> color_eyre::Result<()> {
    let mut term = signal(SignalKind::terminate()).wrap_err("install SIGTERM handler")?;
    let mut interrupt = signal(SignalKind::interrupt()).wrap_err("install SIGINT handler")?;
    tokio::select! {
        _ = term.recv() => {}
        _ = interrupt.recv() => {}
    }
    Ok(())
}

/// 解析 daemon 监听用的 unix socket 路径,顺手把 `runtime_dir()` 创建出来。
fn socket_path() -> color_eyre::Result<std::path::PathBuf> {
    let dir = mineral_paths::runtime_dir().wrap_err("resolve runtime_dir")?;
    std::fs::create_dir_all(&dir).wrap_err_with(|| format!("mkdir -p {}", dir.display()))?;
    Ok(dir.join("mineral.sock"))
}

/// 检测 stale socket。
///
/// - 不存在 → OK,直接返回
/// - 存在 → 试 connect:
///   - 连得上 → daemon 已活,bail
///   - 连不上(ConnectionRefused / NotFound)→ 残留 socket 文件,删
async fn prepare_socket(path: &std::path::Path) -> color_eyre::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    match UnixStream::connect(path).await {
        Ok(_) => bail!(
            "another mineral daemon is already running at {}",
            path.display()
        ),
        Err(_) => {
            std::fs::remove_file(path)
                .wrap_err_with(|| format!("remove stale socket {}", path.display()))?;
            Ok(())
        }
    }
}
