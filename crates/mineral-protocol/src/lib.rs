//! Mineral client ↔ server IPC 协议。
//!
//! 协议形态:
//! - **transport**: tokio `UnixStream`(由 caller 接);`tokio_util::codec::LengthDelimitedCodec`
//!   做 framing(4-byte BE 长度前缀 + payload)
//! - **payload encoding**: `bincode` v1
//! - **req/resp**: 同一条 stream 上严格 1:1 顺序;client 发 [`Request`],等收 [`Response`]
//! - **错误**: server 端处理异常用 [`Response::Error`] 兜底;不再额外的 Status code
//!
//! 当前**不支持**多路复用 / 异步推送 (server 主动推 event)。下一段 4b/4c 再升级。

mod cancel;
mod codec;
mod message;

pub use cancel::{CancelFilter, ChannelFetchKindTag};
pub use codec::{Framed, framed, recv, send};
pub use message::{Request, Response};
