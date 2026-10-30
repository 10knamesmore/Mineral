//! 终端图片协议、能力探测与 backend 生命周期状态。

mod backend;
mod protocol;
mod query;

pub(crate) use backend::{TerminalBackend, TerminalGraphics};
pub(crate) use protocol::{GraphicsProtocol, TerminalRelay};
