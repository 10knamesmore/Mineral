//! 客户端会话核心:请求入有界 channel、结果配对与分流、自动合批、独立 reader/writer、
//! 订阅分片组装与断连收束。
//!
//! 设计要点:
//! - **不凑批**:writer 等到第一件事后只吸干当前已排队的命令,不设固定攒批窗口。
//! - **先登记再发送**:请求的结果去向在写线前进入在途表,慢 writer 不阻塞 reader。
//! - **有界**:待发命令、在途请求、事件缓冲、PCM 窗口、分片组装各自限额;超限明确失败。
//! - **断连即收束**:等待结论的调用方得到「结果未知」,不伪造业务默认值,也不自动重发。

mod assembly;
mod lifecycle;
mod reader;
mod results;
mod writer;

pub(crate) use lifecycle::{Session, SessionHandle, SessionShared, open_session};
pub(crate) use results::ResultTarget;
