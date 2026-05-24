//! 网易云三种加密的实现:[`weapi`] / [`eapi`] / [`linuxapi`]。
//!
//! 三个公开入口都返回 `application/x-www-form-urlencoded` 的 form body 字符串,
//! 可以直接作为 `POST` 的 body 发出。

/// AES (CBC/PKCS7、ECB/PKCS7) 实现,供 weapi/linuxapi/eapi 共用。
mod aes;
pub mod constants;
/// eapi(EAPI / `/eapi/...`)加密入口。
mod eapi;
/// linuxapi(`/api/linux/forward`)加密入口。
mod linuxapi;
/// 16 字节随机 secret key 生成器(weapi 二段 AES key)。
mod rand16;
/// RSA no-padding 加密(weapi 用于把 secret key 包成 `encSecKey`)。
mod rsa;
/// weapi(`/weapi/...`)加密入口。
mod weapi;

pub use eapi::eapi;
pub use linuxapi::linuxapi;
pub use weapi::weapi;

// 暴露给 tests/ 的内部 helper(`pub` 是 crate-public,通过 doc(hidden) 表示非稳定 API)
#[doc(hidden)]
pub mod __internal {
    pub use super::aes::{aes_cbc_pkcs7_encrypt, aes_ecb_pkcs7_encrypt};
    pub use super::rsa::rsa_no_padding_encrypt;
    pub use super::weapi::weapi_with_secret_key;
}
