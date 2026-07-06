//! 进程内一次性 HTTP server,供下载链路测试喂固定响应体。

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 起一个进程内一次性 HTTP server,固定返回 `body`,给出可 GET 的 URL。
///
/// 接受一个连接、排空请求(忽略内容)、回 `200 OK` + `Content-Length` + `body` 后关闭。
///
/// # Params:
///   - `body`: 响应体字节。
///
/// # Return:
///   指向该 server 的 URL。
pub async fn serve_once(body: Vec<u8>) -> color_eyre::Result<url::Url> {
    serve_once_status(/*status*/ 200, body).await
}

/// 同 [`serve_once`],但状态行可指定,供非 2xx 分支(下载失败/回退)测试用。
///
/// # Params:
///   - `status`: HTTP 状态码(如 404)。
///   - `body`: 响应体字节。
///
/// # Return:
///   指向该 server 的 URL。
pub async fn serve_once_status(status: u16, body: Vec<u8>) -> color_eyre::Result<url::Url> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            drop(sock.read(&mut buf).await); // 排空请求(忽略内容)
            let head = format!(
                "HTTP/1.1 {status} Mock\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            drop(sock.write_all(head.as_bytes()).await);
            drop(sock.write_all(&body).await);
            drop(sock.shutdown().await);
        }
    });
    Ok(url::Url::parse(&format!("http://{addr}/a.flac"))?)
}
