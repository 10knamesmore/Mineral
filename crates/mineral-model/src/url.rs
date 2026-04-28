use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use url::Url;

/// 媒体资源(封面、播放流、音频文件)的位置,在类型层面区分远端/本地。
///
/// 有意不实现 `Default`——上层应当显式选 `Remote`/`Local`,避免"空 URL"这种语义。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MediaUrl {
    /// 远端 URL,scheme 通常是 `http`/`https`。
    Remote(Url),
    /// 本地文件路径。
    Local(PathBuf),
}

impl MediaUrl {
    /// 远端便捷构造。`s` 必须是合法 URL,否则返回 `Err`。
    pub fn remote(s: &str) -> Result<Self, url::ParseError> {
        Ok(Self::Remote(Url::parse(s)?))
    }

    /// 本地便捷构造。
    pub fn local(p: impl Into<PathBuf>) -> Self {
        Self::Local(p.into())
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Remote(_))
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }

    /// 取远端 URL,本地变体返回 `None`。
    pub fn as_remote(&self) -> Option<&Url> {
        match self {
            Self::Remote(u) => Some(u),
            Self::Local(_) => None,
        }
    }

    /// 取本地路径,远端变体返回 `None`。
    pub fn as_local(&self) -> Option<&Path> {
        match self {
            Self::Local(p) => Some(p),
            Self::Remote(_) => None,
        }
    }
}

impl fmt::Display for MediaUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Remote(u) => fmt::Display::fmt(u, f),
            Self::Local(p) => fmt::Display::fmt(&p.display(), f),
        }
    }
}

/// 解析规则:
/// - `http://` / `https://` 等可以被 `url::Url` 解析的 → [`MediaUrl::Remote`]
/// - `file://<path>` → [`MediaUrl::Local`](去掉 `file://` 前缀)
/// - 其他 → 当作本地路径
impl FromStr for MediaUrl {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(rest) = s.strip_prefix("file://") {
            return Ok(Self::Local(PathBuf::from(rest)));
        }
        if let Ok(u) = Url::parse(s) {
            // 排除 url crate 把 windows 路径 "C:\..." 当成 scheme 的歧义:
            // 只接受常见的网络/资源 scheme。
            match u.scheme() {
                "http" | "https" | "ftp" | "ftps" | "data" => return Ok(Self::Remote(u)),
                _ => {}
            }
        }
        Ok(Self::Local(PathBuf::from(s)))
    }
}

impl Serialize for MediaUrl {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Remote(u) => s.serialize_str(u.as_str()),
            // 本地路径序列化时统一加 `file://` 前缀,以便反序列化时识别
            Self::Local(p) => s.serialize_str(&format!("file://{}", p.display())),
        }
    }
}

impl<'de> Deserialize<'de> for MediaUrl {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(d)?;
        Ok(s.parse().expect("MediaUrl::from_str is infallible"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_as_remote() {
        let u: MediaUrl = "https://example.com/a.jpg".parse().unwrap();
        assert!(u.is_remote());
    }

    #[test]
    fn parses_file_scheme_as_local() {
        let u: MediaUrl = "file:///home/me/a.flac".parse().unwrap();
        assert!(u.is_local());
        assert_eq!(u.as_local().unwrap(), Path::new("/home/me/a.flac"));
    }

    #[test]
    fn parses_bare_path_as_local() {
        let u: MediaUrl = "/home/me/a.flac".parse().unwrap();
        assert!(u.is_local());
    }

    #[test]
    fn serde_remote_roundtrip() {
        let u = MediaUrl::remote("https://x.y/z").unwrap();
        let s = serde_json::to_string(&u).unwrap();
        assert_eq!(s, "\"https://x.y/z\"");
        let back: MediaUrl = serde_json::from_str(&s).unwrap();
        assert_eq!(back, u);
    }

    #[test]
    fn serde_local_roundtrip() {
        let u = MediaUrl::local("/tmp/a.mp3");
        let s = serde_json::to_string(&u).unwrap();
        assert_eq!(s, "\"file:///tmp/a.mp3\"");
        let back: MediaUrl = serde_json::from_str(&s).unwrap();
        assert_eq!(back, u);
    }
}
