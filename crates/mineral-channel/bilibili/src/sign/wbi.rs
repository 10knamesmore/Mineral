//! WBI 签名算法:`mixin_key` 重排 + query 编码 + `w_rid` 计算。
//!
//! 算法(对齐官方 wbi 规范):
//! 1. `mixin_key` = `img_key + sub_key` 拼成 64 位串,按 [`MIXIN_KEY_ENC_TAB`] 重排后**取前 32
//!    字符**(重排即得,**不**套 md5)。
//! 2. 请求参数加 `wts`(unix 秒),按 key 字典序拼 query,值经 [`wbi_encode`] 百分号编码。
//! 3. `w_rid = md5(sorted_query + mixin_key)`(小写 hex)。

use md5::{Digest, Md5};

/// `mixin_key` 重排索引表(官方固定 64 项);取前 32 项组成 `mixin_key`。
const MIXIN_KEY_ENC_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

/// 由 `img_key` / `sub_key` 派生 `mixin_key`。
///
/// 把两个 key 拼成 64 位串,按 [`MIXIN_KEY_ENC_TAB`] 重排后取前 32 字符。索引经 `get` 取,
/// 越界项(理论上不会,key 恒 32 位)静默跳过,不 panic。
///
/// # Params:
///   - `img_key`: 从 `nav` 的 `wbi_img.img_url` 文件名取得(32 位 hex)
///   - `sub_key`: 从 `nav` 的 `wbi_img.sub_url` 文件名取得(32 位 hex)
///
/// # Return:
///   32 字符的 `mixin_key`。
pub fn mixin_key(img_key: &str, sub_key: &str) -> String {
    let concat = format!("{img_key}{sub_key}");
    let bytes = concat.as_bytes();
    MIXIN_KEY_ENC_TAB
        .iter()
        .take(32)
        .filter_map(|&i| bytes.get(i).copied().map(char::from))
        .collect()
}

/// WBI query 值编码:保留 `A-Za-z0-9-_.~`,剔除 `!'()*`,其余字节百分号编码(大写 hex)。
///
/// 按字节遍历,故 UTF-8(如中文关键词)被逐字节正确编码——签名必须建立在编码后的 query 上,
/// 否则服务端算出的 `w_rid` 不一致(返 `-352`)。
///
/// # Params:
///   - `s`: 原始 key 或 value
///
/// # Return:
///   编码后的字符串。
pub fn wbi_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else if matches!(b, b'!' | b'\'' | b'(' | b')' | b'*') {
            // 官方算法剔除这五个字符(不编码、不保留)。
        } else {
            out.push('%');
            out.push(hex_upper_nibble(b >> 4));
            out.push(hex_upper_nibble(b & 0x0f));
        }
    }
    out
}

/// 半字节(0..=15)→ 大写 hex 字符。
fn hex_upper_nibble(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        _ => char::from(b'A' + nibble - 10),
    }
}

/// 对参数签名,返回带 `wts` + `w_rid` 的完整 query 串。
///
/// `wts` 显式传入(而非内部取时间)以便 test vector 钉死;生产侧由 [`sign`] 包一层取当前时间。
///
/// # Params:
///   - `params`: 业务参数(不含 `wts`/`w_rid`);函数内部会追加 `wts` 并按 key 排序
///   - `img_key` / `sub_key`: WBI keys
///   - `wts`: unix 秒时间戳
///
/// # Return:
///   形如 `a=1&b=2&wts=...&w_rid=...` 的已签名 query。
pub fn sign_with_wts(
    mut params: Vec<(&str, String)>,
    img_key: &str,
    sub_key: &str,
    wts: u64,
) -> String {
    let key = mixin_key(img_key, sub_key);
    params.push(("wts", wts.to_string()));
    params.sort_by(|a, b| a.0.cmp(b.0));
    let query = params
        .iter()
        .map(|(k, v)| format!("{}={}", wbi_encode(k), wbi_encode(v)))
        .collect::<Vec<String>>()
        .join("&");
    let w_rid = md5_hex(&format!("{query}{key}"));
    format!("{query}&w_rid={w_rid}")
}

/// md5(小写 hex)。
fn md5_hex(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// 从 WBI 图片 URL 提取 key(即文件名去扩展名)。
///
/// 如 `https://i0.hdslb.com/bfs/wbi/7cd0...c.png` → `7cd0...c`。
///
/// # Params:
///   - `url`: `nav` 返回的 `img_url` / `sub_url`
///
/// # Return:
///   提取到的 key;URL 无 `/` 或无扩展名时 `None`。
pub fn extract_key(url: &str) -> Option<String> {
    url.rsplit_once('/')
        .and_then(|(_, file)| file.rsplit_once('.'))
        .map(|(stem, _)| stem.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{extract_key, mixin_key, sign_with_wts};

    /// mixin_key 重排 test vector(官方文档示例):钉死重排算法正确性。
    #[test]
    fn mixin_key_matches_official_vector() {
        let key = mixin_key(
            "7cd084941338484aae1ad9425b84077c",
            "4932caff0ff746eab6f01bf08b70ac45",
        );
        assert_eq!(key, "ea1db124af3c7062474693fa704f4ff8");
    }

    /// 完整签名 test vector(官方文档示例,`wts=1702204169`):钉死排序 + 编码 + w_rid = md5。
    /// w_rid 算错服务端返 -352 且 HTTP 仍 200,排查成本高,故 byte-for-byte 钉死。
    #[test]
    fn sign_matches_official_vector() {
        let signed = sign_with_wts(
            vec![
                ("foo", "114".to_owned()),
                ("bar", "514".to_owned()),
                ("zab", "1919810".to_owned()),
            ],
            "7cd084941338484aae1ad9425b84077c",
            "4932caff0ff746eab6f01bf08b70ac45",
            1702204169,
        );
        assert_eq!(
            signed,
            "bar=514&foo=114&wts=1702204169&zab=1919810&w_rid=8f6f2b5b3d485fe1886cec6a0be8c5d4"
        );
    }

    /// 中文关键词按 UTF-8 逐字节百分号编码(大写 hex),签名建立在编码后 query 上。
    #[test]
    fn sign_percent_encodes_utf8_keyword() {
        let signed = sign_with_wts(
            vec![("keyword", "周杰伦".to_owned())],
            "7cd084941338484aae1ad9425b84077c",
            "4932caff0ff746eab6f01bf08b70ac45",
            1702204169,
        );
        // 周杰伦 的 UTF-8 百分号编码(大写)。
        assert!(
            signed.starts_with("keyword=%E5%91%A8%E6%9D%B0%E4%BC%A6&wts=1702204169&w_rid="),
            "实际:{signed}"
        );
    }

    /// 从 WBI 图片 URL 提取 key(文件名去扩展名)。
    #[test]
    fn extract_key_from_wbi_url() {
        assert_eq!(
            extract_key("https://i0.hdslb.com/bfs/wbi/7cd084941338484aae1ad9425b84077c.png"),
            Some("7cd084941338484aae1ad9425b84077c".to_owned())
        );
        assert_eq!(extract_key("no-slash-no-dot"), None);
    }
}
