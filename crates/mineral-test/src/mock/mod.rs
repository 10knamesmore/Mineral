//! 测试 mock 的命名空间:每类 mock 各占一文件,按需扩充。
//!
//! - [`crate::mock::serve_once`] / [`crate::mock::serve_once_status`]:进程内一次性 HTTP server。
//! - [`crate::mock::UrlChannel`]:返回固定直链的 mock channel。
//! - [`crate::mock::DetailChannel`]:`songs_detail` 返回预置曲目的 mock channel。
//! - [`crate::mock::CannedChannel`]:`album_detail` / `lyrics` 返回罐头数据并计歌词调用数的 mock channel。

mod channel;
mod http;

pub use channel::{CannedChannel, DetailChannel, UrlChannel};
pub use http::{serve_once, serve_once_status};
