//! 网易云三种加密的实现:[`weapi`] / [`eapi`] / [`linuxapi`]。
//!
//! 三个公开入口都返回 `application/x-www-form-urlencoded` 的 form body 字符串,
//! 可以直接作为 `POST` 的 body 发出。
//!
//! 实现严格对照 `musicfox/netease-api-rust-spec.md` §1,自检 harness 在
//! `tests/crypto_vectors.rs`(用 openssl 作参考实现,做 byte-for-byte 比对)。

mod aes;
pub mod constants;
mod eapi;
mod linuxapi;
mod rand16;
mod rsa;
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
