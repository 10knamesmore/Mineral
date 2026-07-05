//! 哔哩哔哩 channel:WBI 签名 + 搜索 / 详情 / 播放 URL 取流。
//!
//! B站是明文 REST——鉴权靠 cookie(SESSDATA/bili_jct)+ WBI 签名(对 query 参数做 md5 派生
//! `w_rid`),没有网易那样的 AES 加密请求体。故不设 `crypto/`/`device/` 层,签名逻辑集中在
//! [`sign`]。

pub mod api;
pub mod channel;
pub mod cli;
pub mod config;
pub mod convert;
pub mod credential;
pub mod error;
pub mod sign;
pub mod transport;
pub mod wire;

pub use channel::BilibiliChannel;
pub use config::BilibiliConfig;
pub use credential::{StoredBilibiliAuth, load_stored};
