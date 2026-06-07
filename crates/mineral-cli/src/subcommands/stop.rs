//! `mineral stop` — 请求后台 daemon 优雅退出([`Request::Shutdown`]),
//! `mineral status` 的对偶。
//!
//! 语义是「**确保** daemon 不在跑」:daemon 本就没跑时幂等成功(exit 0),
//! 脚本里可无脑调用。返回前轮询 socket 文件消失——返回即收尾真完成,
//! 紧接着 `mineral serve` 不会撞 stale socket。

use std::time::{Duration, Instant};

use color_eyre::eyre::bail;
use mineral_protocol::{OneshotClient, Request, Response};

/// 等 daemon 收尾(unlink socket)的上限。收尾通常亚秒;慢机 / CI 留宽裕。
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

/// `mineral stop` 入口:连 daemon socket → 发 Shutdown → 等 socket 消失。
///
/// # Return:
///   daemon 已退出(或本就没跑)返回 `Ok`;daemon 在跑但进不去(busy / 版本
///   不匹配)或没在期限内退出返回 `Err`。
pub async fn run() -> color_eyre::Result<()> {
    let socket_path = mineral_paths::socket_path()?;
    // 连接与握手分两步,错误分类不同:连不上 = daemon 不在跑 → 幂等成功;
    // 握手被拒(busy / 版本不匹配)= daemon 在跑但停不了 → 报错冒泡。
    let Ok(stream) = tokio::net::UnixStream::connect(&socket_path).await else {
        println!("没有在跑的 daemon");
        return Ok(());
    };
    let mut client = OneshotClient::from_stream(stream).await?;
    match client.request(Request::Shutdown).await {
        Ok(Response::Ok) => {}
        Ok(other) => bail!("unexpected response: {other:?}"),
        // ack 是尽力而为:daemon 收到请求即开始收尾,应答可能没写完连接就关了。
        // EOF / 写失败不当失败,由下面的 socket 消失轮询裁决。
        Err(_) => {}
    }
    let deadline = Instant::now() + EXIT_TIMEOUT;
    while socket_path.exists() {
        if Instant::now() >= deadline {
            bail!(
                "daemon 没有在 {EXIT_TIMEOUT:?} 内退出(socket {} 仍在)",
                socket_path.display()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    println!("daemon 已停止");
    Ok(())
}
