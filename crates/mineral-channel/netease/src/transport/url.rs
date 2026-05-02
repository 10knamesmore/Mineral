use once_cell::sync::Lazy;
use regex::Regex;

const BASE_URL: &str = "https://music.163.com";

static API_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"/\w*api/").unwrap());

/// 加密类型。决定 URL 改写到哪个端点。
#[derive(Clone, Copy, Debug)]
pub enum Crypto {
    Weapi,
    Eapi,
    Linuxapi,
}

/// 按 spec §2.1 改写 URL。
///
/// - `Crypto::Weapi` → `/\w*api/` 替换为 `/weapi/`
/// - `Crypto::Eapi`  → `/\w*api/` 替换为 `/eapi/`
/// - `Crypto::Linuxapi` → 整段替换为 `https://music.163.com/api/linux/forward`
///
/// 返回完整 URL(含 host)。
pub fn rewrite(path: &str, crypto: Crypto) -> String {
    if matches!(crypto, Crypto::Linuxapi) {
        return format!("{BASE_URL}/api/linux/forward");
    }

    let target = match crypto {
        Crypto::Weapi => "/weapi/",
        Crypto::Eapi => "/eapi/",
        Crypto::Linuxapi => unreachable!(),
    };

    let rewritten = if API_REGEX.is_match(path) {
        API_REGEX.replace(path, target).into_owned()
    } else {
        path.to_owned()
    };

    if rewritten.starts_with("http://") || rewritten.starts_with("https://") {
        rewritten
    } else {
        format!("{BASE_URL}{rewritten}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weapi_rewrites_api_to_weapi() {
        let out = rewrite("/api/song/detail", Crypto::Weapi);
        assert_eq!(out, "https://music.163.com/weapi/song/detail");
    }

    #[test]
    fn eapi_rewrites_weapi_to_eapi() {
        let out = rewrite("/weapi/song/lyric/v1", Crypto::Eapi);
        assert_eq!(out, "https://music.163.com/eapi/song/lyric/v1");
    }

    #[test]
    fn linuxapi_replaces_full_url() {
        let out = rewrite("/anything/here", Crypto::Linuxapi);
        assert_eq!(out, "https://music.163.com/api/linux/forward");
    }

    #[test]
    fn keeps_absolute_url_for_non_linuxapi() {
        let out = rewrite("https://interface3.music.163.com/eapi/x/y", Crypto::Eapi);
        assert_eq!(out, "https://interface3.music.163.com/eapi/x/y");
    }
}
