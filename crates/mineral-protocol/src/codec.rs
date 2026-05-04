//! Bincode + length-delimited framing helper。
//!
//! 上层只用 [`framed`] / [`send`] / [`recv`] 三个 API,看不到 bytes-level 编码。

use bytes::{Bytes, BytesMut};
use color_eyre::eyre::WrapErr;
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Framed as TokioFramed, LengthDelimitedCodec};

/// 带 length-delimited framing 的双向流。包 [`tokio::net::UnixStream`] 或测试用的
/// `tokio::io::DuplexStream` 都可以。
pub type Framed<T> = TokioFramed<T, LengthDelimitedCodec>;

/// 用 length-delimited codec 包一个 stream。
pub fn framed<T: AsyncRead + AsyncWrite>(stream: T) -> Framed<T> {
    LengthDelimitedCodec::new().framed(stream)
}

/// 把一条 serde-serializable 消息编码成 [`Bytes`] 发出去。
///
/// # Errors
/// bincode 序列化失败 / 写 stream 失败。
pub async fn send<T, S>(stream: &mut Framed<S>, msg: &T) -> color_eyre::Result<()>
where
    T: Serialize,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let bytes = bincode::serialize(msg).wrap_err("bincode encode")?;
    stream
        .send(Bytes::from(bytes))
        .await
        .wrap_err("framed send")
}

/// 收一条消息并 bincode 反序列化。
///
/// # Errors
/// stream 关闭返回 `Ok(None)`(EOF);其它 I/O 错误 / 解码错误返回 `Err`。
pub async fn recv<T, S>(stream: &mut Framed<S>) -> color_eyre::Result<Option<T>>
where
    T: DeserializeOwned,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Some(frame) = stream.next().await else {
        return Ok(None);
    };
    let frame: BytesMut = frame.wrap_err("framed recv")?;
    let msg: T = bincode::deserialize(&frame).wrap_err("bincode decode")?;
    Ok(Some(msg))
}
