//! 加密自检 harness。
//!
//! 用 openssl 作为"参考实现",和本 crate 的纯 Rust 加密三件套做 byte-for-byte 比对。
//! 任何位错都会让这些测试爆,从而保证我们的实现和服务端能解出来的输入完全一致。

// reason: 测试 harness 中常规使用 unwrap / as / format! 等,与 crate 主体一致放开。
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::as_conversions,
    clippy::cast_lossless,
    clippy::format_push_string,
    clippy::uninlined_format_args,
    clippy::redundant_closure_for_method_calls
)]

use mineral_channel_netease::crypto::__internal::{
    aes_cbc_pkcs7_encrypt, aes_ecb_pkcs7_encrypt, rsa_no_padding_encrypt, weapi_with_secret_key,
};
use mineral_channel_netease::crypto::{eapi, linuxapi};

use openssl::{
    rsa::{Padding, Rsa},
    symm::{encrypt, Cipher},
};

const PRESET_KEY: &[u8] = b"0CoJUm6Qyw8W8jud";
const IV: &[u8] = b"0102030405060708";
const LINUX_API_KEY: &[u8] = b"rFgB&h#%2?^eDg:Q";
const EAPI_KEY: &[u8] = b"e82ckenh8dichen8";
const RSA_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDgtQn2JZ34ZC28NWYpAUd98iZ3
7BUrX/aKzmFbt7clFSs6sXqHauqKWqdtLkF2KexO40H1YTX8z2lSgBBOAxLsvaklV
8k4cBFK9snQXE9/DDaFt6Rr7iVZMldczhC0JNgTz+SHXT6CBHuX3e9SdB1Ua44on
caTWz7OBGLbCiK45wIDAQAB
-----END PUBLIC KEY-----";

fn ref_aes_cbc(plaintext: &[u8], key: &[u8], iv: &[u8]) -> Vec<u8> {
    encrypt(Cipher::aes_128_cbc(), key, Some(iv), plaintext).unwrap()
}

fn ref_aes_ecb(plaintext: &[u8], key: &[u8]) -> Vec<u8> {
    encrypt(Cipher::aes_128_ecb(), key, None, plaintext).unwrap()
}

fn ref_rsa_no_padding(secret_key: &[u8; 16]) -> Vec<u8> {
    let rsa = Rsa::public_key_from_pem(RSA_PEM.as_bytes()).unwrap();
    let mut buf = vec![0u8; 128 - 16];
    buf.extend_from_slice(secret_key);
    let mut out = vec![0u8; rsa.size() as usize];
    rsa.public_encrypt(&buf, &mut out, Padding::NONE).unwrap();
    out
}

#[test]
fn aes_cbc_matches_openssl() {
    let plaintexts: &[&[u8]] = &[
        b"",
        b"hello",
        b"\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\x0f", // exactly 1 block
        br#"{"id":"123","csrf_token":""}"#,
        b"a much longer plaintext that spans many AES blocks to make sure padding and chaining are right",
    ];
    let preset_key: [u8; 16] = PRESET_KEY.try_into().unwrap();
    let iv: [u8; 16] = IV.try_into().unwrap();
    for pt in plaintexts {
        let ours = aes_cbc_pkcs7_encrypt(pt, &preset_key, &iv);
        let theirs = ref_aes_cbc(pt, PRESET_KEY, IV);
        assert_eq!(ours, theirs, "AES-CBC mismatch for {pt:?}");
    }
}

#[test]
fn aes_ecb_matches_openssl_for_eapi_key() {
    let plaintexts: &[&[u8]] = &[
        b"",
        b"abcdefghijklmnop", // exact one block
        b"hello world",
        br#"{"foo":"bar","header":{"os":"ios"}}"#,
    ];
    let key: [u8; 16] = EAPI_KEY.try_into().unwrap();
    for pt in plaintexts {
        let ours = aes_ecb_pkcs7_encrypt(pt, &key);
        let theirs = ref_aes_ecb(pt, EAPI_KEY);
        assert_eq!(ours, theirs, "EAPI AES-ECB mismatch for {pt:?}");
    }
}

#[test]
fn aes_ecb_matches_openssl_for_linuxapi_key() {
    let plaintexts: &[&[u8]] = &[
        b"",
        b"hello",
        br#"{"method":"POST","url":"/api/song/detail","params":{"ids":"[123]"}}"#,
    ];
    let key: [u8; 16] = LINUX_API_KEY.try_into().unwrap();
    for pt in plaintexts {
        let ours = aes_ecb_pkcs7_encrypt(pt, &key);
        let theirs = ref_aes_ecb(pt, LINUX_API_KEY);
        assert_eq!(ours, theirs, "LINUXAPI AES-ECB mismatch for {pt:?}");
    }
}

#[test]
fn rsa_no_padding_matches_openssl() {
    let keys: &[[u8; 16]] = &[
        *b"AAAAAAAAAAAAAAAA",
        *b"0123456789abcdef",
        *b"abcdef0123456789",
        // 高熵随机字符
        *b"q9Z3kPxL2VmH8nWb",
    ];
    for sk in keys {
        let ours = rsa_no_padding_encrypt(sk);
        let theirs = ref_rsa_no_padding(sk);
        assert_eq!(ours, theirs, "RSA no-padding mismatch for {:?}", sk);
        assert_eq!(ours.len(), 128, "RSA output should be exactly 128 bytes");
    }
}

#[test]
fn weapi_form_body_matches_reference() {
    // 用固定的 secret_key / re_secret_key 消除随机性,然后构造一份"参考"输出
    // 让两边都跑同样的算法,确保结果一致。
    let json = br#"{"s":"hello","type":"1","offset":"0","limit":"30","csrf_token":""}"#;
    // 注意:re_secret_key 是 secret_key 的字符反序;此处用全 'A',正反一样。
    let sk: [u8; 16] = *b"AAAAAAAAAAAAAAAA";
    let rsk: [u8; 16] = *b"AAAAAAAAAAAAAAAA";

    // 我们的实现
    let ours = weapi_with_secret_key(std::str::from_utf8(json).unwrap(), &sk, &rsk);

    // 用 openssl 重新算一份,作为参考
    use base64::Engine;
    let inner = ref_aes_cbc(json, PRESET_KEY, IV);
    let inner_b64 = base64::engine::general_purpose::STANDARD.encode(&inner);
    let outer = ref_aes_cbc(inner_b64.as_bytes(), &rsk, IV);
    let params = base64::engine::general_purpose::STANDARD.encode(&outer);
    let enc_sec = ref_rsa_no_padding(&sk);
    let enc_sec_hex = hex::encode(&enc_sec);

    // 期望输出格式与本 crate 的 weapi 函数一致(form-urlencoded)
    let expected = format!(
        "params={}&encSecKey={}",
        urlencode_compatible(&params),
        urlencode_compatible(&enc_sec_hex),
    );

    assert_eq!(ours, expected);
}

#[test]
fn linuxapi_form_body_matches_reference() {
    let json = r#"{"method":"POST","url":"/api/song/detail","params":{"ids":"[123]"}}"#;
    let ours = linuxapi(json);

    let cipher = ref_aes_ecb(json.as_bytes(), LINUX_API_KEY);
    let hex_upper = hex::encode_upper(&cipher);
    let expected = format!("eparams={}", urlencode_compatible(&hex_upper));
    assert_eq!(ours, expected);
}

#[test]
fn eapi_form_body_matches_reference() {
    use md5::{Digest, Md5};

    let logical = "/api/song/lyric/v1";
    let text = r#"{"id":"123","cp":"false","header":{"os":"ios"}}"#;
    let ours = eapi(logical, text);

    // 参考实现:严格按 spec §1.3
    let message = format!("nobody{logical}use{text}md5forencrypt");
    let mut hasher = Md5::new();
    hasher.update(message.as_bytes());
    let digest = hex::encode(hasher.finalize());
    let data = format!("{logical}-36cd479b6b5-{text}-36cd479b6b5-{digest}");
    let cipher = ref_aes_ecb(data.as_bytes(), EAPI_KEY);
    let hex_upper = hex::encode_upper(&cipher);
    let expected = format!("params={}", urlencode_compatible(&hex_upper));

    assert_eq!(ours, expected);
}

/// 与 weapi/eapi/linuxapi 模块内部 urlencode 行为一致的实现。
fn urlencode_compatible(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{byte:02X}"));
            }
        }
    }
    out
}
