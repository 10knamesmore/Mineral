//! `mineral stop` — 请求后台 daemon 优雅退出,版本握手不兼容时向 socket peer 发 SIGTERM;
//! `mineral status` 的对偶。
//!
//! 语义是「**确保** daemon 不在跑」:daemon 本就没跑时幂等成功(exit 0),
//! 脚本里可无脑调用。返回前轮询 socket 文件消失——返回即收尾真完成,
//! 紧接着 `mineral serve` 不会撞 stale socket。

use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use mineral_client::Client;
use mineral_client::connection::{ClientConfig, ConnectError};
use mineral_protocol::{RejectReason, SocketWire};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use tokio::net::UnixStream;

/// 等 daemon 收尾(unlink socket)的上限。收尾通常亚秒;慢机 / CI 留宽裕。
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

/// `mineral stop` 入口:连 daemon socket → 发 Shutdown → 等 socket 消失。
///
/// # Return:
///   daemon 已退出(或本就没跑)返回 `Ok`;无法确认 / 终止 daemon,或没在期限内
///   退出返回 `Err`。
pub async fn run() -> Result<()> {
    let socket_path = mineral_paths::socket_path()?;
    // 连不上 = daemon 不在跑 → 幂等成功。连上后优先走会话协议;只有 daemon 明确
    // 回 VersionMismatch 才 signal 内核确认的 socket peer,避免误杀无关进程。
    let stream = match UnixStream::connect(&socket_path).await {
        Ok(stream) => stream,
        Err(source)
            if matches!(
                source.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            println!("no daemon running");
            return Ok(());
        }
        Err(source) => {
            return Err(Error::Io {
                operation: "connect to daemon for shutdown",
                path: socket_path,
                source,
            });
        }
    };
    request_shutdown_with(stream, &socket_path, send_sigterm).await?;
    wait_for_exit(&socket_path).await?;
    println!("daemon stopped");
    Ok(())
}

/// 优先经会话协议请求关停;握手明确因版本错配被拒时,向 Unix socket peer 发信号。
///
/// `signal_peer` 作为参数让测试能验证目标 pid 而不真的终止测试进程。
///
/// # Params:
///   - `stream`: 已连接到 daemon socket 的流。
///   - `socket_path`: 已连接 daemon 的 socket 路径，用于失败诊断。
///   - `signal_peer`: 向已确认 peer pid 投递关停信号的实现。
///
/// # Return:
///   shutdown 请求已经通过会话协议或进程信号投递。
async fn request_shutdown_with<F>(
    stream: UnixStream,
    socket_path: &std::path::Path,
    signal_peer: F,
) -> Result<()>
where
    F: FnOnce(i32) -> Result<()>,
{
    // 必须在会话层消费 stream 前读取;版本拒绝后 daemon 会主动关连接。
    let peer_pid = stream.peer_cred().map(|credentials| credentials.pid());
    let wire = SocketWire::from_stream(stream);
    let client =
        match Client::from_wire(Box::new(wire), "mineral_stop", ClientConfig::default()).await {
            Ok(client) => client,
            Err(ConnectError::Rejected(rejected)) => {
                if rejected.reason() != Some(RejectReason::VersionMismatch) {
                    return Err(Error::Handshake {
                        path: socket_path.to_path_buf(),
                        source: ConnectError::Rejected(rejected),
                    });
                }
                let pid = peer_pid
                    .map_err(|source| Error::Signal {
                        operation: "read daemon socket peer credentials",
                        source,
                    })?
                    .ok_or(Error::MissingPeerPid)?;
                signal_peer(pid)?;
                return Ok(());
            }
            Err(error) => {
                return Err(Error::Handshake {
                    path: socket_path.to_path_buf(),
                    source: error,
                });
            }
        };
    // ack 是尽力而为:daemon 收到请求即开始收尾,应答可能没写完连接就关了。
    // `Unknown` 不当失败,由下面的 socket 消失轮询裁决。
    let _ = client.shutdown().await;
    Ok(())
}

/// 向 daemon pid 发送 SIGTERM,复用 daemon 已安装的 graceful shutdown 路径。
///
/// # Params:
///   - `pid`: Unix socket peer 的进程 id。
fn send_sigterm(pid: i32) -> Result<()> {
    kill(Pid::from_raw(pid), Signal::SIGTERM).map_err(|source| Error::Kill { pid, source })
}

/// 等 daemon 收尾并 unlink socket,确保返回后可以立即重新启动。
///
/// # Params:
///   - `socket_path`: daemon socket 路径。
async fn wait_for_exit(socket_path: &std::path::Path) -> Result<()> {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    while socket_path.exists() {
        if Instant::now() >= deadline {
            return Err(Error::ExitTimeout {
                timeout: EXIT_TIMEOUT,
                path: socket_path.to_path_buf(),
            });
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use color_eyre::eyre::eyre;
    use mineral_protocol::{
        MessageBatch, RejectReason, ServerHello, SessionMessage, SocketWire, Wire,
    };

    use super::request_shutdown_with;

    /// 版本握手被拒时,stop 应从已连接 socket 取得 daemon pid 并走 SIGTERM fallback。
    #[tokio::test]
    async fn version_mismatch_falls_back_to_peer_signal() -> color_eyre::Result<()> {
        let (client_stream, server_stream) = tokio::net::UnixStream::pair()?;
        let server = tokio::spawn(async move {
            let mut wire = SocketWire::from_stream(server_stream);
            let batch = wire
                .recv()
                .await?
                .ok_or_else(|| eyre!("server 没收到握手"))?;
            assert!(
                matches!(batch.messages.first(), Some(SessionMessage::Hello(_))),
                "首帧应为 Hello,实际 {batch:?}"
            );
            wire.send(MessageBatch::one(SessionMessage::Welcome(
                ServerHello::reject(RejectReason::VersionMismatch),
            )))
            .await?;
            Ok::<(), color_eyre::Report>(())
        });
        let signaled_pid = Cell::<Option<i32>>::new(None);

        request_shutdown_with(
            client_stream,
            std::path::Path::new("test-daemon.sock"),
            |pid| {
                signaled_pid.set(Some(pid));
                Ok(())
            },
        )
        .await?;

        let current_pid = i32::try_from(std::process::id())?;
        assert_eq!(
            signaled_pid.get(),
            Some(current_pid),
            "fallback 必须 signal socket 对端进程"
        );
        server.await??;
        Ok(())
    }
}
