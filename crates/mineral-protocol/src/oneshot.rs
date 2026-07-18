//! 一次性 client:连接 + 握手 + 串行 request/response 配对的最小封装。
//!
//! 给 CLI 一次性命令(`mineral status` 等)用 —— 不订阅任何推送、不起后台
//! worker,发一条等一条;间隙里交错下来的 [`Frame::Event`] 直接跳过。
//! 长连接交互式 client(TUI)不用它,走自己的 worker(id 配对 + event 通道)。

use std::path::Path;

use color_eyre::eyre::{WrapErr, bail};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::UnixStream;

use crate::codec::{Framed, framed, recv, send};
use crate::frame::{Frame, RequestId};
use crate::handshake::ClientInfo;
use crate::message::{Request, Response};

/// 本类型握手自报的 client 名:oneshot 即「CLI 一次性命令」的封装(见模块
/// 文档),其他形态的 client 走各自的长连接实现、自报各自的名。
const CLIENT_NAME: &str = "mineral_cli";

/// 一次性串行 client。泛型 stream 仅为可测性(`tokio::io::duplex`);
/// 生产路径经 [`OneshotClient::connect`] 固定为 `UnixStream`。
pub struct OneshotClient<S = UnixStream> {
    /// 已完成握手的连接。
    conn: Framed<S>,

    /// 下一个请求 id(自增)。
    next_id: u64,
}

impl OneshotClient<UnixStream> {
    /// 连接 daemon socket 并完成握手(订阅空集)。
    ///
    /// # Params:
    ///   - `socket_path`: daemon 的 unix socket 路径
    ///
    /// # Return:
    ///   已可发请求的 client。
    ///
    /// # Errors
    /// 连接失败 / 握手被拒(busy、版本不匹配 —— 错误信息已是人话提示)。
    pub async fn connect(socket_path: &Path) -> color_eyre::Result<Self> {
        let stream = UnixStream::connect(socket_path).await.wrap_err_with(|| {
            format!(
                "connect daemon socket {} (run `mineral serve` first?)",
                socket_path.display()
            )
        })?;
        Self::from_stream(stream).await
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> OneshotClient<S> {
    /// 在已建立的双向流上完成握手(订阅空集),返回可发请求的 client。
    ///
    /// # Params:
    ///   - `stream`: 已连接的双向流
    ///
    /// # Errors
    /// 握手被拒 / 对端没回 [`Frame::Hello`] / 连接被关。
    pub async fn from_stream(stream: S) -> color_eyre::Result<Self> {
        let mut conn = framed(stream);
        crate::handshake::client_handshake(&mut conn, ClientInfo::new(CLIENT_NAME, Vec::new()))
            .await?;
        Ok(Self { conn, next_id: 0 })
    }

    /// 发一条请求并等待**配对**的应答;间隙里交错的 [`Frame::Event`] 跳过。
    ///
    /// # Params:
    ///   - `req`: 请求体
    ///
    /// # Return:
    ///   配对的应答。
    ///
    /// # Errors
    /// 连接被关 / 收到无法配对的帧。
    pub async fn request(&mut self, req: Request) -> color_eyre::Result<Response> {
        let id = RequestId::new(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        send(&mut self.conn, &Frame::Request { id, req })
            .await
            .wrap_err("发送请求失败")?;
        loop {
            match recv::<Frame, _>(&mut self.conn)
                .await
                .wrap_err("等待应答")?
            {
                Some(Frame::Response { id: got, resp }) if got == id => return Ok(*resp),
                Some(Frame::Event(_)) => {
                    // 一次性命令订空集,正常不会有 event;容忍并跳过。
                }
                Some(other) => bail!("收到无法配对的帧:{other:?}"),
                None => bail!("daemon 在应答前关闭了连接"),
            }
        }
    }
}
