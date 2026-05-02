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
//! - [`wire`] —— 网易原生 JSON 结构(serde)
//! - [`api`] —— 端点封装(逻辑层)
//! - [`channel`] —— 把 `api/` 的方法绑到 `MusicChannel` trait

// reason: 这个 crate 是网易私有 API 的实现细节(crypto / dto / transport),
// 内部 pub item 上百个,逐一写文档对外部使用者价值低于函数/结构体名本身。
// 公共入口 `NeteaseChannel` / `NeteaseConfig` 在各自文件里有详细文档。
#![allow(missing_docs)]
// reason: 加密层(AES / RSA / 字节切片操作)需要大量 `as` 与按下标访问,
// 在 crypto 实测代码里这些是必需且正确的;在 transport 解压层也类似。
// 整 crate 放开比逐函数写 allow + reason 更清晰。
#![allow(
    clippy::as_conversions,
    clippy::cast_lossless,
    clippy::indexing_slicing,
    clippy::index_refutable_slice
)]
// reason: 网易 API 调用栈深、返回结构复杂,内部 unwrap / expect 在以下场景使用:
// (1) 静态字典 / 已校验过的常量解析,(2) tests 与 examples,
// (3) 加密字节流的不变量(例如 16-byte 块对齐已保证)。逐点 allow 噪音过大。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// reason: 内部多种"形似 Option<Option<T>>"的 DTO 字段(网易 JSON 设计如此),
// serde 必须保留这种嵌套以区分"字段缺失" vs "字段为 null"。
#![allow(clippy::option_option)]
#![cfg_attr(test, allow(clippy::needless_pass_by_value, clippy::implicit_clone,))]

pub mod api;
pub mod channel;
pub mod cli;
pub mod config;
pub mod credential;
pub mod crypto;
pub mod device;
pub mod transport;
pub mod wire;

pub use credential::{StoredNeteaseAuth, load_stored};

pub mod convert;

pub use channel::NeteaseChannel;
pub use config::NeteaseConfig;
