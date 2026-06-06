//! IPC 顶层帧:request-id 配对 + [`Event`](crate::Event) 同连接交错下推的 wire 形状。

use serde::{Deserialize, Serialize};

use crate::{ClientInfo, Event, Request, Response, ServerHello};

/// 单调请求标识。client 自增分配,server 在对应 [`Frame::Response`] 原样回带,
/// 用于在交错的 Event 流里把 reply 配对回发起方的请求。
///
/// 只在单条连接内有意义(断链即退出,无跨连接陈旧 id 问题)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RequestId(u64);

impl RequestId {
    /// 用裸值构造(client 侧自增计数器单入口)。
    #[must_use]
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// 裸值,日志 / 调试用。
    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

/// IPC 连接上**唯一**过 codec 的顶层帧。一条连接上 client→server 与 server→client
/// 双向都发 `Frame`;`Request`/`Response` 经 [`RequestId`] 配对,`Event` 可在任意
/// 时刻交错下推(不占 reply 槽)。
///
/// 连接建立后 client 必须先发 [`Frame::Handshake`],等到 [`Frame::Hello`]
/// (`accepted == true`)后才可发 [`Frame::Request`]。
///
/// codec 无关:本类型只 derive `Serialize`/`Deserialize`,bincode(今)与 JSON
/// (将来)切换不改本定义(守卫见 `tests/frame.rs` 双 codec round-trip)。
#[derive(Debug, Serialize, Deserialize)]
pub enum Frame {
    /// client → server:握手首帧(先于任何 [`Frame::Request`])。
    Handshake(ClientInfo),

    /// server → client:握手应答(版本守门 / busy 拒绝的结果)。
    Hello(ServerHello),

    /// client → server:带 id 的请求。
    Request {
        /// 配对标识。
        id: RequestId,

        /// 请求体(沿用既有 [`Request`])。
        req: Request,
    },

    /// server → client:带 id 的应答,`id` 原样回带发起的 [`Frame::Request`]。
    Response {
        /// 与对应请求相同的标识。
        id: RequestId,

        /// 应答体(沿用既有 [`Response`];`Box` 避免 enum 体积膨胀)。
        resp: Box<Response>,
    },

    /// server → client:主动推送,不配对 id;按握手订阅集过滤。
    Event(Event),
}
