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

/// daemon 入口。需要 caller 已 `tokio::Runtime::block_on` 或在 async ctx 中调。
pub async fn run(channels: Vec<Arc<dyn MusicChannel>>) -> color_eyre::Result<()> {
    let socket_path = socket_path()?;
    prepare_socket(&socket_path).await?;
    let listener = UnixListener::bind(&socket_path)
        .wrap_err_with(|| format!("bind unix socket {}", socket_path.display()))?;
    println!("mineral daemon listening on {}", socket_path.display());

    let server = Server::spawn(channels)?;
    server.serve(listener).await
}

fn socket_path() -> color_eyre::Result<std::path::PathBuf> {
    let dir = mineral_paths::runtime_dir().wrap_err("resolve runtime_dir")?;
    std::fs::create_dir_all(&dir)
        .wrap_err_with(|| format!("mkdir -p {}", dir.display()))?;
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
