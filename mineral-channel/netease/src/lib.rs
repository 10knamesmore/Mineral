//! 网易云音乐 channel 实现。
//!
//! 公共出口:
//! - [`NeteaseChannel`] —— [`mineral_channel_core::MusicChannel`] 的具体实现
//! - [`config::NeteaseConfig`] —— 客户端构造参数
//!
//! 模块架构(自底向上):
//! - [`crypto`] —— WEAPI / EAPI / LINUXAPI 三种加密(纯 Rust,有自检 harness)
//! - [`device`] —— deviceId 池、sDeviceId、ChainID 等设备指纹
//! - [`transport`] —— HTTP 请求 dispatcher,负责 URL 改写、UA、cookie、zlib 解压
//! - [`dto`] —— 网易原生 JSON 结构(serde)
//! - [`api`] —— 端点封装(逻辑层)
//! - [`channel`] —— 把 `api/` 的方法绑到 `MusicChannel` trait

pub mod api;
pub mod channel;
pub mod config;
pub mod crypto;
pub mod device;
pub mod dto;
pub mod transport;

pub mod convert;

pub use channel::NeteaseChannel;
pub use config::NeteaseConfig;
