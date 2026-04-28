use base64::Engine;

use super::aes::aes_cbc_pkcs7_encrypt;
use super::constants::{IV, PRESET_KEY};
use super::rand16::new_len16_rand;
use super::rsa::rsa_no_padding_encrypt;

/// WEAPI 加密入口(spec §1.1)。
///
/// 输入是一段 JSON 文本(已经包含业务参数 + `csrf_token`),输出是
/// `application/x-www-form-urlencoded` 形式的 form body 字符串,
/// 即 `params=...&encSecKey=...`。
pub fn weapi(json_text: &str) -> String {
    let (sk, rsk) = new_len16_rand();
    weapi_with_secret_key(json_text, &sk, &rsk)
}

/// 把 secret_key 暴露成参数的版本,**仅供测试/自检 harness 用**(消除随机性)。
///
/// 调用方必须保证 `re_secret_key` 是 `secret_key` 的字符反序——否则会和
/// 服务端解密逻辑不一致。
#[doc(hidden)]
pub fn weapi_with_secret_key(
    json_text: &str,
    secret_key: &[u8; 16],
    re_secret_key: &[u8; 16],
) -> String {
    let inner = aes_cbc_pkcs7_encrypt(json_text.as_bytes(), PRESET_KEY, IV);
    let inner_b64 = base64::engine::general_purpose::STANDARD.encode(&inner);
    let outer = aes_cbc_pkcs7_encrypt(inner_b64.as_bytes(), re_secret_key, IV);
    let params = base64::engine::general_purpose::STANDARD.encode(&outer);

    let enc_sec_bytes = rsa_no_padding_encrypt(secret_key);
    let enc_sec_key = hex::encode(&enc_sec_bytes);

    form_urlencode(&[("params", &params), ("encSecKey", &enc_sec_key)])
}

fn form_urlencode(pairs: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        out.push_str(&urlencode(k));
        out.push('=');
        out.push_str(&urlencode(v));
    }
    out
}

fn urlencode(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}
