use md5::{Digest, Md5};

use super::aes::aes_ecb_pkcs7_encrypt;
use super::constants::{
    EAPI_KEY, EAPI_MD5_INFIX, EAPI_MD5_PREFIX, EAPI_MD5_SUFFIX, EAPI_SEPARATOR,
};

/// EAPI 加密入口(spec §1.3)。
///
/// `url_logical_path` 必须用 service 的"逻辑路径"(`Options.Url`,例如
/// `/api/song/lyric/v1`)而不是请求实际打到的 `/eapi/...`——MD5 包裹时用的就是
/// 逻辑路径,改错会导致服务端校验失败。
///
/// `json_text` 是已经包含业务参数 + `header` 字段的 JSON 文本。
pub fn eapi(url_logical_path: &str, json_text: &str) -> String {
    // 1. message = "nobody" + url + "use" + text + "md5forencrypt"
    let message =
        format!("{EAPI_MD5_PREFIX}{url_logical_path}{EAPI_MD5_INFIX}{json_text}{EAPI_MD5_SUFFIX}");

    // 2. md5 hex 小写
    let mut hasher = Md5::new();
    hasher.update(message.as_bytes());
    let digest = hex::encode(hasher.finalize());

    // 3. data = url + sep + text + sep + digest
    let data = format!("{url_logical_path}{EAPI_SEPARATOR}{json_text}{EAPI_SEPARATOR}{digest}");

    // 4. AES-128-ECB + PKCS7 + hex 大写
    let cipher = aes_ecb_pkcs7_encrypt(data.as_bytes(), EAPI_KEY);
    let params = hex::encode_upper(&cipher);

    format!("params={}", urlencode(&params))
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
