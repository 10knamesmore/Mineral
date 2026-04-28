use super::aes::aes_ecb_pkcs7_encrypt;
use super::constants::LINUX_API_KEY;

/// LINUXAPI 加密入口(spec §1.2)。
///
/// `json_text` 是 `{"method":"POST","url":"...","params":{...}}` 序列化后的文本。
/// 输出是 `eparams=<HEX_UPPER>` 形式的 form body。
pub fn linuxapi(json_text: &str) -> String {
    let cipher = aes_ecb_pkcs7_encrypt(json_text.as_bytes(), LINUX_API_KEY);
    let params = hex::encode_upper(&cipher);
    format!("eparams={}", urlencode(&params))
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
