//! 结构化会话消息的传输接口、传输错误及 Unix socket 实现。

mod error;
mod socket;
mod transport;

pub use error::WireError;
pub use socket::SocketWire;
pub use transport::{Wire, WireSink, WireSource};
