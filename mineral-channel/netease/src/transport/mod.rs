//! HTTP 请求 dispatcher。
//!
//! 包含:
//! - [`client`]:isahc HttpClient + cookie jar 单例
//! - [`headers`]:UA 池、固定 header 注入
//! - [`url`]:URL 改写正则(spec §2.1)
//! - [`body`]:zlib 解压、`code` 字段解析

pub mod body;
pub mod client;
pub mod headers;
pub mod url;

pub use client::Transport;
