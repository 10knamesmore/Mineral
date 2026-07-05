//! nav 端点(`x/web-interface/nav`)DTO:取 WBI keys。
//!
//! guest(未登录)请求 nav 返回 `code = -101`,但 `data.wbi_img` **仍然提供**——故取 keys 时
//! 走「不校验 code」的 lax 路径,只读 `data.wbi_img`。

use serde::Deserialize;

/// nav 响应的 `data` 主体(登录态等字段略,只取 `wbi_img`)。
#[derive(Debug, Clone, Deserialize)]
pub struct NavData {
    /// WBI 图片 URL 对(从文件名提取 `img_key`/`sub_key`)。
    pub wbi_img: WbiImg,
}

/// WBI 图片 URL 对。
#[derive(Debug, Clone, Deserialize)]
pub struct WbiImg {
    /// `img_key` 来源 URL。
    pub img_url: String,

    /// `sub_key` 来源 URL。
    pub sub_url: String,
}
