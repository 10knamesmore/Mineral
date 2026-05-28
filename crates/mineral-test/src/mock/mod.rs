//! 测试 mock 的命名空间:每类 mock 各占一文件,按需扩充。
//!
//! - [`serve_once`]:进程内一次性 HTTP server。
//! - [`UrlChannel`]:返回固定直链的 mock channel。

mod channel;
mod http;

pub use channel::UrlChannel;
pub use http::serve_once;
