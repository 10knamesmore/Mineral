//! Bincode + length-delimited framing helper。
//!
//! 上层只用 [`framed`] / [`send`] / [`recv`] 三个 API,看不到 bytes-level 编码。

use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Decoder, Framed as TokioFramed, LengthDelimitedCodec};

/// 消息编解码与帧读写失败。
#[derive(Debug, Error)]
pub enum CodecError {
    /// 消息无法序列化成 bincode 负载。
    #[error("编码消息失败")]
    Encode(#[source] Box<bincode::ErrorKind>),

    /// 收到的字节不是有效的 bincode 消息。
    #[error("解码消息失败")]
    Decode(#[source] Box<bincode::ErrorKind>),

    /// 帧写入失败。
    #[error("写入消息帧失败")]
    Send(#[source] std::io::Error),

    /// 帧读取失败。
    #[error("读取消息帧失败")]
    Receive(#[source] std::io::Error),

    /// 帧长超过 length-delimited codec 接受的最大值。
    #[error("消息帧超过 {max} 字节上限")]
    FrameTooLarge {
        /// codec 接受的最大负载长度。
        max: usize,

        /// 包含原始帧解析错误的 I/O 错误。
        #[source]
        source: std::io::Error,
    },
}

/// 从 I/O 错误中识别 length-delimited codec 的帧长超限。
fn oversized_frame(source: &std::io::Error) -> bool {
    source.get_ref().is_some_and(
        <dyn std::error::Error + Send + Sync>::is::<tokio_util::codec::LengthDelimitedCodecError>,
    )
}

/// 将 length-delimited 帧读取错误按具体原因分类。
pub(crate) fn receive_error(source: std::io::Error) -> CodecError {
    if oversized_frame(&source) {
        CodecError::FrameTooLarge {
            max: LengthDelimitedCodec::new().max_frame_length(),
            source,
        }
    } else {
        CodecError::Receive(source)
    }
}

/// 将 length-delimited 帧写入错误按具体原因分类。
pub(crate) fn send_error(source: std::io::Error) -> CodecError {
    if oversized_frame(&source) {
        CodecError::FrameTooLarge {
            max: LengthDelimitedCodec::new().max_frame_length(),
            source,
        }
    } else {
        CodecError::Send(source)
    }
}

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
pub fn encode<T: Serialize>(msg: &T) -> Result<Bytes, CodecError> {
    let mut bytes = Vec::new();
    bincode::serialize_into(&mut bytes, msg).map_err(CodecError::Encode)?;
    Ok(Bytes::from(bytes))
}

/// 从一帧负载字节解码消息([`encode`] 的对偶)。
///
/// # Params:
///   - `frame`: 一帧负载(已被 [`Framed`] 剥掉长度前缀)
///
/// # Errors
/// bincode 反序列化失败。
pub fn decode<T: DeserializeOwned>(frame: &[u8]) -> Result<T, CodecError> {
    bincode::deserialize(frame).map_err(CodecError::Decode)
}

/// 把一条 serde-serializable 消息编码成 [`Bytes`] 发出去。
///
/// 用 `serialize_into` 直写 `Vec` 单遍完成:`bincode::serialize` 内部会先跑一遍
/// SizeChecker 预算长度再真序列化,大 payload 等于序列化两遍(profiling 可见)。
///
/// # Errors
/// bincode 序列化失败 / 写 stream 失败。
pub async fn send<T, S>(stream: &mut Framed<S>, msg: &T) -> Result<(), CodecError>
where
    T: Serialize,
    S: AsyncRead + AsyncWrite + Unpin,
{
    stream.send(encode(msg)?).await.map_err(send_error)
}

/// 收一条消息并 bincode 反序列化。
///
/// # Errors
/// stream 关闭返回 `Ok(None)`(EOF);其它 I/O 错误 / 解码错误返回 `Err`。
pub async fn recv<T, S>(stream: &mut Framed<S>) -> Result<Option<T>, CodecError>
where
    T: DeserializeOwned,
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Some(frame) = stream.next().await else {
        return Ok(None);
    };
    let frame: BytesMut = frame.map_err(receive_error)?;
    Ok(Some(decode(&frame)?))
}
