//! 网易原生 DTO → `mineral_model` 类型的转换 helper。

use mineral_model::MediaUrl;

/// 把网易 JSON 里的字符串字段(永远是 http(s) URL)转成 `MediaUrl::Remote`。
///
/// 解析失败或空字符串返回 `None`,让 `Option<MediaUrl>` 字段保持空。
pub fn parse_remote(s: &str) -> Option<MediaUrl> {
    if s.is_empty() {
        return None;
    }
    MediaUrl::remote(s).ok()
}

/// `Option<&str>` 版的便利包装。
pub fn parse_remote_opt(s: Option<&str>) -> Option<MediaUrl> {
    s.and_then(parse_remote)
}
