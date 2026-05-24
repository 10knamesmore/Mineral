//! `mineral serve` — 起 daemon,bind unix socket,跑 `Server::serve` accept loop。
//!
//! 调用方(main.rs)负责 build channels 后传进来。本模块负责:
//! 1. 解析 socket 路径
//! 2. stale socket 检测(已活 daemon → bail;残留 socket 文件 → 删)
//! 3. bind + Server::spawn + serve

use std::sync::Arc;

use color_eyre::eyre::{WrapErr, bail};
use mineral_channel_core::MusicChannel;
use mineral_server::{AudioMode, Server};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{Signal, SignalKind, signal};

/// daemon 入口。需要 caller 已 `tokio::Runtime::block_on` 或在 async ctx 中调。
///
/// accept loop 与关闭信号(SIGINT / SIGTERM)竞争:收到信号时主动停掉 server
/// (走 [`Server::shutdown`] 的 Drop 链)并 unlink socket 文件,避免残留 stale
/// socket 留给下次启动清理。
pub async fn run(channels: Vec<Arc<dyn MusicChannel>>) -> color_eyre::Result<()> {
    mineral_log::info!(target: "daemon", "starting mineral daemon");
    // 信号 handler 必须在 bind 之前装好:unix socket 一 bind,client 就能连上(连接进
    // backlog,无需 accept),并可能立刻请求退出。若此刻 handler 还没装(Server::spawn
    // 的 audio 初始化耗时不短),SIGTERM 会走默认处置直接杀进程 —— stale socket 残留、
    // audio / MPRIS 收尾全跳过。提前装好即可关掉这段竞态窗口。
    let mut term = signal(SignalKind::terminate()).wrap_err("install SIGTERM handler")?;
    let mut interrupt = signal(SignalKind::interrupt()).wrap_err("install SIGINT handler")?;

    let socket_path = mineral_paths::socket_path()?;
    prepare_socket(&socket_path).await?;
    let listener = UnixListener::bind(&socket_path)
        .wrap_err_with(|| format!("bind unix socket {}", socket_path.display()))?;
    mineral_log::info!(target: "daemon", socket_path = %socket_path.display(), "unix socket bound");
    println!("mineral daemon listening on {}", socket_path.display());

    // `MINERAL_AUDIO_NULL` 强制 null 后端(无设备 e2e / headless 确定性复现降级);
    // 未设则 Auto(有设备真出声,无设备自动降级)。env 只在 binary 边缘读,lib 保持纯。
    let audio_mode = if std::env::var_os("MINERAL_AUDIO_NULL").is_some() {
        AudioMode::ForceNull
    } else {
        AudioMode::Auto
    };
    let server = Server::spawn(channels, audio_mode)?;
    mineral_log::info!(target: "daemon", "server core initialized");
    // 接入系统媒体服务(MPRIS)。无 D-Bus session 等失败时降级:daemon 照常跑。
    if let Err(e) = server.start_media_service() {
        mineral_log::warn!(target: "media", error = mineral_log::chain(&e), "system media service unavailable");
    }
    let outcome = tokio::select! {
        result = server.serve(listener) => result,
        () = wait_for_signal(&mut term, &mut interrupt) => {
            mineral_log::info!(target: "daemon", "shutdown signal received, stopping daemon");
            Ok(())
        }
    };

    // graceful 收尾:停 server(Drop 链停 audio engine / scheduler)+ 清 socket。
    mineral_log::info!(target: "daemon", "shutting down");
    server.shutdown();
    if let Err(e) = std::fs::remove_file(&socket_path) {
        mineral_log::warn!(
            target: "daemon",
            socket_path = %socket_path.display(),
            error = mineral_log::chain(&e),
            "remove socket on shutdown failed"
        );
    } else {
        mineral_log::debug!(target: "daemon", socket_path = %socket_path.display(), "socket cleaned up");
    }
    outcome
}

/// 等待第一个到达的关闭信号(SIGINT 或 SIGTERM)。
///
/// handler 由 caller 在 bind socket **之前**就装好(`signal(...)` 调用时即安装),
/// 这里只 `recv` 等触发,避免「socket 已可连但 handler 未就绪」的竞态。
///
/// # Params:
///   - `term`: 已安装的 SIGTERM 信号流。
///   - `interrupt`: 已安装的 SIGINT 信号流。
async fn wait_for_signal(term: &mut Signal, interrupt: &mut Signal) {
    tokio::select! {
        _ = term.recv() => {}
        _ = interrupt.recv() => {}
    }
}

/// 检测 stale socket。
///
/// - 不存在 → OK,直接返回
/// - 存在 → 试 connect:
///   - 连得上 → daemon 已活,bail
///   - 连不上(ConnectionRefused / NotFound)→ 残留 socket 文件,删
async fn prepare_socket(path: &std::path::Path) -> color_eyre::Result<()> {
    if !path.exists() {
        mineral_log::debug!(target: "daemon", "socket path fresh, no cleanup needed");
        return Ok(());
    }
    match UnixStream::connect(path).await {
        Ok(_) => {
            mineral_log::error!(target: "daemon", socket_path = %path.display(), "another daemon already running");
            bail!(
                "another mineral daemon is already running at {}",
                path.display()
            )
        }
        Err(_) => {
            mineral_log::warn!(target: "daemon", socket_path = %path.display(), "removing stale socket");
            std::fs::remove_file(path)
                .wrap_err_with(|| format!("remove stale socket {}", path.display()))?;
            Ok(())
        }
    }
}
