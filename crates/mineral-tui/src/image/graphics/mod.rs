//! 启动时确定图片协议与传输方式；字符像素尺寸独立查询。

mod protocol;
mod query;
mod session;

pub(crate) use protocol::{GraphicsProtocol, TerminalRelay};
pub(crate) use query::{query_cell_pixels, window_cell_pixels};
pub(crate) use session::TerminalGraphics;
