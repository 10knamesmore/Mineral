//! HTTP 传输层:isahc 客户端 + cookie jar + WBI 签名 GET dispatch。
//!
//! 与网易的差别:B站是明文 REST + GET,鉴权靠 cookie + WBI 签名(见 [`crate::sign`]),故无
//! 加密 body / 路径改写;固定头只有 `Referer` + 浏览器 UA(见 [`headers`])。

pub mod client;
pub mod headers;

pub use client::Transport;
