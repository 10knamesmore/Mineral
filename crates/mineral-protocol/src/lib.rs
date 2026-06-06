//! Mineral client ↔ server IPC 协议。
//!
//! 协议形态:
//! - **transport**: tokio `UnixStream`(由 caller 接);`tokio_util::codec::LengthDelimitedCodec`
//!   做 framing(4-byte BE 长度前缀 + payload)
//! - **payload encoding**: `bincode` v1(wire 类型只依赖 serde derive,codec 可换,
//!   守卫见 `tests/frame.rs` 双 codec round-trip)
//! - **顶层帧**: [`Frame`] —— 连接上唯一过 codec 的类型。client 先发
//!   [`Frame::Handshake`](版本守门 + 订阅集),server 回 [`Frame::Hello`];之后
//!   [`Frame::Request`]/[`Frame::Response`] 经 [`RequestId`] 配对,server 可在任意
//!   时刻交错下推 [`Frame::Event`](按订阅集过滤)。
//! - **版本守门**: 无协商 —— 两端包版本([`PkgVersion`])相等才互通,
//!   错配回 `Hello { accepted: false }`,client 提示重启 daemon。
//! - **错误**: server 端处理异常用 [`Response::Error`] 兜底;不再额外的 Status code

mod cancel;
mod codec;
mod event;
mod frame;
mod handshake;
mod message;
mod oneshot;
mod player;

pub use cancel::CancelFilter;
pub use codec::{Framed, decode, encode, framed, recv, send};
pub use event::{Event, FinishReason, PropName, PropValue, ToastKind};
pub use frame::{Frame, RequestId};
pub use handshake::{
    ClientInfo, PkgVersion, RejectReason, ServerHello, Subscription, client_handshake,
};
pub use message::{DownloadProgress, DownloadTarget, Request, Response, SongStatsWire};
pub use mineral_task::ChannelFetchKindTag;
pub use oneshot::OneshotClient;
pub use player::{
    CurrentSync, PlayMode, PlaybackOrigin, PlayerSync, PlayerVersions, QueueSync, Repeat,
};
