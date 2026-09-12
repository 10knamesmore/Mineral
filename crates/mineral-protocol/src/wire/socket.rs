//! Unix socket 承载:bincode + length-delimited framing 的 adapter。

use async_trait::async_trait;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use tokio::net::UnixStream;

use crate::{Framed, MessageBatch, framed};

use super::{Wire, WireError, WireSink, WireSource};

/// Unix socket 上的会话承载。
pub struct SocketWire {
    /// 已建立的双向 framed 连接。
    conn: Framed<UnixStream>,
}

impl SocketWire {
    /// 连接 daemon socket(不做握手;握手是会话层的事)。
    ///
    /// # Params:
    ///   - `path`: daemon unix socket 路径
    ///
    /// # Errors
    /// 连接失败(保留 `ErrorKind` 供不可达判定)。
    pub async fn connect(path: &std::path::Path) -> Result<Self, WireError> {
        let stream = UnixStream::connect(path)
            .await
            .map_err(|source| WireError::Io {
                operation: "连接 socket",
                source,
            })?;
        Ok(Self {
            conn: framed(stream),
        })
    }

    /// 包一条已连接的流(测试 / 已握有 socket 的场景)。
    ///
    /// # Params:
    ///   - `stream`: 已连接的 Unix 流
    #[must_use]
    pub fn from_stream(stream: UnixStream) -> Self {
        Self {
            conn: framed(stream),
        }
    }
}

#[async_trait]
impl Wire for SocketWire {
    async fn send(&mut self, batch: MessageBatch) -> Result<(), WireError> {
        self.conn
            .send(encode_batch(&batch)?)
            .await
            .map_err(|source| WireError::Io {
                operation: "写入 socket",
                source,
            })
    }

    async fn recv(&mut self) -> Result<Option<MessageBatch>, WireError> {
        let Some(frame) = self.conn.next().await else {
            return Ok(None);
        };
        let frame = frame.map_err(|source| WireError::Io {
            operation: "读取 socket",
            source,
        })?;
        Ok(Some(decode_batch(&frame)?))
    }

    async fn close(&mut self) -> Result<(), WireError> {
        self.conn.close().await.map_err(|source| WireError::Io {
            operation: "关闭 socket",
            source,
        })
    }

    fn split(self: Box<Self>) -> (Box<dyn WireSink>, Box<dyn WireSource>) {
        let (sink, stream) = self.conn.split();
        (
            Box::new(SocketSink { sink }),
            Box::new(SocketSource { stream }),
        )
    }
}

/// socket 承载的发送半。
struct SocketSink {
    /// framed 写半。
    sink: SplitSink<Framed<UnixStream>, bytes::Bytes>,
}

#[async_trait]
impl WireSink for SocketSink {
    async fn send(&mut self, batch: MessageBatch) -> Result<(), WireError> {
        self.sink
            .send(encode_batch(&batch)?)
            .await
            .map_err(|source| WireError::Io {
                operation: "写入 socket",
                source,
            })
    }

    async fn close(&mut self) -> Result<(), WireError> {
        self.sink.close().await.map_err(|source| WireError::Io {
            operation: "关闭 socket",
            source,
        })
    }
}

/// socket 承载的接收半。
struct SocketSource {
    /// framed 读半。
    stream: SplitStream<Framed<UnixStream>>,
}

#[async_trait]
impl WireSource for SocketSource {
    async fn recv(&mut self) -> Result<Option<MessageBatch>, WireError> {
        let Some(frame) = self.stream.next().await else {
            return Ok(None);
        };
        let frame = frame.map_err(|source| WireError::Io {
            operation: "读取 socket",
            source,
        })?;
        Ok(Some(decode_batch(&frame)?))
    }
}

/// 把一批会话消息编成一帧负载；编解码失败归为协议错误。
///
/// # Params:
///   - `batch`: 待发送消息。
///
/// # Errors
/// bincode 序列化失败。
fn encode_batch(batch: &MessageBatch) -> Result<bytes::Bytes, WireError> {
    crate::encode(batch).map_err(|source| WireError::Protocol {
        operation: "编码会话批",
        source,
    })
}

/// 将已去掉长度前缀的一帧负载解码为会话消息。
///
/// # Params:
///   - `frame`: 一帧负载。
///
/// # Errors
/// bincode 反序列化失败。
fn decode_batch(frame: &[u8]) -> Result<MessageBatch, WireError> {
    crate::decode::<MessageBatch>(frame).map_err(|source| WireError::Protocol {
        operation: "解码会话批",
        source,
    })
}
