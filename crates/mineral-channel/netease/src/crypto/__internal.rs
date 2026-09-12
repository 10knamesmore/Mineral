//! 供加密向量测试使用的加密原语与固定密钥入口。

pub use super::aes::{aes_cbc_pkcs7_encrypt, aes_ecb_pkcs7_encrypt};
pub use super::rsa::rsa_no_padding_encrypt;
pub use super::weapi::weapi_with_secret_key;
