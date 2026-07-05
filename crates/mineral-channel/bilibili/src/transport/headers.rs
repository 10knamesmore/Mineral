//! B站请求的固定头。

/// 浏览器 User-Agent(部分端点 + 取流 CDN 校验;非浏览器 UA 会被拒)。
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/120.0.0.0 Safari/537.36";

/// 固定 Referer:API 请求与音频 `baseUrl` 取流都要带,否则 403。
pub const REFERER: &str = "https://www.bilibili.com";
