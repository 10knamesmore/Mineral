//! Mineral client ↔ daemon 会话协议。
//!
//! 协议形态:
//! - **逻辑会话**:一次连接 = 一条会话;两端交换 [`SessionMessage`],传输与编码由
//!   [`Wire`] adapter 决定。握手是会话首帧 [`SessionMessage::Hello`] / `Welcome`。
//! - **请求配对**:[`SessionRequest::id`](RequestId)由 client 分配,daemon 在
//!   [`SessionResult`] 原样回带;应答可乱序,同方向按会话次序交付。
//! - **订阅推送**:[`SubscriptionTopic`] 声明订阅,daemon 用带 `version` 的
//!   [`UpdateEnvelope`] 推增量;大载荷按 `(subscription, version)` 分片组装。
//! - **编码**:当前 socket adapter 用 bincode v1 + length-delimited framing;wire
//!   类型只依赖 serde derive,codec 可换(守卫见 `tests/session_codec.rs`)。
//! - **版本守门**:[`PkgVersion::compatible_with`] 决定两端包版本是否互通;
//!   错配回拒绝的 `Welcome`,client 提示重启 daemon。
//! - **错误**:业务失败用结构化 [`OperationFailure`],查询载荷用 [`Response`]。

mod codec;
mod download;
mod event;
mod handshake;
mod key;
mod message;
mod player;
mod queue_edit;
mod session;
mod store;
mod wire;

pub use codec::{Framed, decode, encode, framed, recv, send};
pub use download::{
    DownloadId, DownloadOrigin, DownloadStatus, DownloadSummary, DownloadTarget, DownloadWave,
    SongDownloadView,
};
pub use event::{
    BusValue, Event, FinishReason, PropName, PropValue, SpanAlign, SpanFg, TextSpan, ToastKind,
};
pub use handshake::{
    ClientInfo, HandshakeRejected, PkgVersion, RejectReason, ServerHello, Subscription,
};
pub use key::{KeyContext, PlaylistRef, ScriptBind, ViewKind};
pub use message::{
    CopyTemplateCtx, PlayQueueError, QueueContextWire, Request, Response, SongStatsWire,
};
pub use mineral_task::ChannelFetchKindTag;
pub use player::{
    CurrentSync, PlayCursor, PlayMode, PlaybackOrigin, PlayerSync, PlayerVersions, QueueSync,
    Repeat, SegmentVersion,
};
pub use queue_edit::{QueueAnchor, QueueEditOutcome, QueueOp, QueuePos};
pub use session::{
    CloseReason, DOWNLOAD_DETAIL_PART_ROWS, DownloadDetailDelta, DownloadDetailUpdate, FailureKind,
    MessageBatch, OperationFailure, OperationResult, PLAYER_QUEUE_PART_ROWS, PcmChunk, RequestId,
    SessionMessage, SessionRequest, SessionResult, SubscribeRequest, SubscriptionId,
    SubscriptionTopic, UpdateEnvelope, UpdatePayload, assembly_limits, fragment_download_detail,
    fragment_player_update,
};
pub use store::StoreValue;
pub use wire::{SocketWire, Wire, WireError, WireSink, WireSource};
