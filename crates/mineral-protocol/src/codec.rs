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

/// 把消息编码成一帧负载字节(单遍 `serialize_into`,不含长度前缀——那是
/// [`Framed`] 的事)。`Framed` 被 split 成 sink/stream 两半后无法走 [`send`],
/// 两端的 writer/reader task 用本函数 + [`decode`] 手动过 codec。
///
/// # Params:
///   - `msg`: 要编码的消息
///
/// # Errors
/// bincode 序列化失败。
pub fn encode<T: Serialize>(msg: &T) -> color_eyre::Result<Bytes> {
    let mut bytes = Vec::new();
    bincode::serialize_into(&mut bytes, msg).wrap_err("bincode encode")?;
    Ok(Bytes::from(bytes))
}

/// 从一帧负载字节解码消息([`encode`] 的对偶)。
///
/// # Params:
///   - `frame`: 一帧负载(已被 [`Framed`] 剥掉长度前缀)
///
/// # Errors
/// bincode 反序列化失败。
pub fn decode<T: DeserializeOwned>(frame: &[u8]) -> color_eyre::Result<T> {
    bincode::deserialize(frame).wrap_err("bincode decode")
}

/// 把一条 serde-serializable 消息编码成 [`Bytes`] 发出去。
///
/// 用 `serialize_into` 直写 `Vec` 单遍完成:`bincode::serialize` 内部会先跑一遍
/// SizeChecker 预算长度再真序列化,大 payload 等于序列化两遍(profiling 可见)。
///
/// # Errors
/// bincode 序列化失败 / 写 stream 失败。
pub async fn send<T, S>(stream: &mut Framed<S>, msg: &T) -> color_eyre::Result<()>
where
    T: Serialize,
    S: AsyncRead + AsyncWrite + Unpin,
{
    stream.send(encode(msg)?).await.wrap_err("framed send")
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
    Ok(Some(decode(&frame)?))
}
