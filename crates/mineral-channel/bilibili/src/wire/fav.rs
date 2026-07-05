//! 收藏夹端点 DTO。
//!
//! 两个端点:`x/v3/fav/folder/created/list`(我的收藏夹列表,分页、每项带封面)、
//! `x/v3/fav/resource/list`(某收藏夹内容)。都是明文 GET(无 WBI),私密夹 /「我的」列表需登录
//! cookie。注:另有 `created/list-all`(全量不分页)但每项**不返 cover**,故列表走分页版取封面。

use serde::Deserialize;

/// 「我的收藏夹列表」(分页 `created/list`)响应的 `data`。
#[derive(Debug, Clone, Deserialize)]
pub struct FavFolderList {
    /// 收藏夹列表(无 / 未登录 → `None`)。
    pub list: Option<Vec<FavFolder>>,

    /// 是否还有下一页(驱动翻页;缺失 → `false`)。
    #[serde(default)]
    pub has_more: bool,
}

/// 一个收藏夹(folder)元信息。
#[derive(Debug, Clone, Deserialize)]
pub struct FavFolder {
    /// 收藏夹 id(media_id / fid),拉内容时作 `media_id`。
    pub id: i64,

    /// 收藏夹标题。
    pub title: String,

    /// 收藏条目数。
    #[serde(default)]
    pub media_count: i64,

    /// 收藏夹封面 URL(分页 `created/list` 才返、`list-all` 无;可协议相对)。
    #[serde(default)]
    pub cover: Option<String>,

    /// 收藏夹简介。
    #[serde(default)]
    pub intro: Option<String>,
}

/// 「收藏夹内容」响应的 `data`。
#[derive(Debug, Clone, Deserialize)]
pub struct FavResourceList {
    /// 收藏夹元信息(标题 / 计数 / 封面)。
    pub info: Option<FavInfo>,

    /// 收藏的条目(视频),无 → `None`。
    pub medias: Option<Vec<FavMedia>>,

    /// 是否还有下一页(驱动翻页;缺失 → `false`,单页夹的常见形态)。
    #[serde(default)]
    pub has_more: bool,
}

/// 收藏夹元信息。
#[derive(Debug, Clone, Deserialize)]
pub struct FavInfo {
    /// 收藏夹标题。
    pub title: String,

    /// 收藏条目数。
    #[serde(default)]
    pub media_count: i64,

    /// 收藏夹封面 URL(可缺;可能协议相对)。
    #[serde(default)]
    pub cover: Option<String>,

    /// 收藏夹简介。
    #[serde(default)]
    pub intro: Option<String>,
}

/// 收藏夹里的一个条目(视频)。
#[derive(Debug, Clone, Deserialize)]
pub struct FavMedia {
    /// 视频 BV 号(缺失则该项无法定位,convert 丢弃)。
    #[serde(default)]
    pub bvid: Option<String>,

    /// 标题。
    #[serde(default)]
    pub title: Option<String>,

    /// 封面 URL(可能协议相对)。
    #[serde(default)]
    pub cover: Option<String>,

    /// 时长(秒)。**多 P 视频这里是全 BV 各 P 之和**,不是任何单 P 的时长。
    #[serde(default)]
    pub duration: Option<i64>,

    /// 视频分 P 数(实测单 P 为 1)。缺失按单 P 处理。
    #[serde(default)]
    pub page: Option<i64>,

    /// 视频 UP 主(收藏夹条目里叫 `upper`)。
    pub upper: FavUpper,
}

/// 收藏条目的 UP 主。
#[derive(Debug, Clone, Deserialize)]
pub struct FavUpper {
    /// UP 主数字 ID。
    pub mid: i64,

    /// UP 主名。
    #[serde(default)]
    pub name: String,
}
