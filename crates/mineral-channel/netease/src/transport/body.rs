use std::io::Read;

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

/// 解析远端业务 code；缺失时按无效响应处理。
pub fn parse_code(json: &Value) -> crate::Result<i64> {
    json.get("code")
        .and_then(Value::as_i64)
        .ok_or(crate::Error::InvalidData {
            field: "response.code",
        })
}

/// 把 body 字节解码成 JSON Value(尝试 zlib 解压在前)。
pub fn decode_response(bytes: Vec<u8>) -> crate::Result<Value> {
    let bytes = maybe_zlib_decode(bytes);
    serde_json::from_slice::<Value>(&bytes).map_err(|source| crate::Error::Parse {
        context: "response body".to_owned(),
        source: Box::new(source),
    })
}
