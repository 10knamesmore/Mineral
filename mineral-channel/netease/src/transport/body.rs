use std::io::Read;

use anyhow::Result;
use flate2::read::ZlibDecoder;
use serde_json::Value;

/// 尝试 zlib 解压;若解压失败则原样返回(说明本来就没压缩)。
pub fn maybe_zlib_decode(bytes: Vec<u8>) -> Vec<u8> {
    let mut dec = ZlibDecoder::new(&bytes[..]);
    let mut out = Vec::new();
    if dec.read_to_end(&mut out).is_ok() {
        out
    } else {
        bytes
    }
}

/// 解析 `code` 字段;若 JSON 没有 `code` 字段则按 200 处理(spec §2.5)。
pub fn parse_code(json: &Value) -> i64 {
    json.get("code")
        .and_then(|v| v.as_i64())
        .unwrap_or(200)
}

/// 把 body 字节解码成 JSON Value(尝试 zlib 解压在前)。
pub fn decode_response(bytes: Vec<u8>) -> Result<Value> {
    let bytes = maybe_zlib_decode(bytes);
    Ok(serde_json::from_slice(&bytes)?)
}
