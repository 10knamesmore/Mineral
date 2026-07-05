//! UP 主空间两个端点的响应结构:名片(`web-interface/card`)与投稿列表
//! (`space/wbi/arc/search`)。

use serde::Deserialize;

/// 名片响应的 `data` 块。粉丝数以顶层 `follower` 为准(`card.fans` 是同值旧字段,留作兜底)。
#[derive(Debug, Clone, Deserialize)]
pub struct CardResult {
    /// 名片主体;缺失时上层以请求 mid 兜底出最小 Artist。
    #[serde(default)]
    pub card: Option<CardInfo>,

    /// 粉丝数。
    #[serde(default)]
    pub follower: Option<i64>,

    /// 投稿视频总数(每 BV 一张专辑,即 album 总数)。
    #[serde(default)]
    pub archive_count: Option<i64>,
}

/// 名片主体(用户资料)。
#[derive(Debug, Clone, Deserialize)]
pub struct CardInfo {
    /// 用户名。
    #[serde(default)]
    pub name: Option<String>,

    /// 头像 URL(可能协议相对,convert 补 https)。
    #[serde(default)]
    pub face: Option<String>,

    /// 个性签名。
    #[serde(default)]
    pub sign: Option<String>,

    /// 粉丝数(旧字段,顶层 `follower` 缺失时兜底)。
    #[serde(default)]
    pub fans: Option<i64>,
}

/// 投稿列表响应的 `data` 块。
#[derive(Debug, Clone, Deserialize)]
pub struct ArcSearchResult {
    /// 列表容器;缺失视为空页。
    #[serde(default)]
    pub list: Option<ArcSearchList>,
}

/// 投稿列表容器。
#[derive(Debug, Clone, Deserialize)]
pub struct ArcSearchList {
    /// 投稿视频条目。
    #[serde(default)]
    pub vlist: Vec<ArcVideoItem>,
}

/// 单个投稿视频条目。字段普遍可缺,全 `Option` + default——宁可单项在 convert 处
/// 被判不可用,也不让整批反序列化炸掉(与搜索 DTO 同一防御姿势)。
#[derive(Debug, Clone, Deserialize)]
pub struct ArcVideoItem {
    /// 视频 BV 号;缺失则该项无法定位,convert 会丢弃。
    #[serde(default)]
    pub bvid: Option<String>,

    /// 标题(投稿列表不带搜索高亮标签)。
    #[serde(default)]
    pub title: Option<String>,

    /// 封面 URL(可能协议相对)。
    #[serde(default)]
    pub pic: Option<String>,

    /// 简介。
    #[serde(default)]
    pub description: Option<String>,

    /// 时长文本(`"MM:SS"` / `"HH:MM:SS"`),convert 解析为毫秒。
    #[serde(default)]
    pub length: Option<String>,

    /// 投稿时间(Unix epoch 秒)。
    #[serde(default)]
    pub created: Option<i64>,

    /// UP 主数字 ID。
    #[serde(default)]
    pub mid: Option<i64>,

    /// UP 主名。
    #[serde(default)]
    pub author: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{ArcSearchResult, CardResult};
    use crate::wire::de::from_value;

    /// 名片响应解析:card 主体 + 顶层 follower / archive_count 各就各位。
    #[test]
    fn parses_card_result() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "card": { "mid": "12345", "name": "UP主甲", "face": "https://i0.hdslb.com/f.jpg",
                      "sign": "个签", "fans": 3 },
            "follower": 4567, "archive_count": 89
        });
        let r: CardResult = from_value(raw)?;
        mineral_test::assert_snap_debug!("名片响应解析(card 主体 + follower/archive_count)", r);
        Ok(())
    }

    /// 空响应(全字段缺失)→ 全落 None,不报错。
    #[test]
    fn empty_card_is_all_none() -> color_eyre::Result<()> {
        let r: CardResult = from_value(serde_json::json!({}))?;
        assert!(r.card.is_none());
        assert_eq!(r.follower, None);
        assert_eq!(r.archive_count, None);
        Ok(())
    }

    /// 投稿列表解析:vlist 条目各字段就位;缺字段项(仅 bvid)不炸整批。
    #[test]
    fn parses_arc_search_result() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "list": { "vlist": [
                { "bvid": "BV1xx", "title": "投稿一", "pic": "//i0.hdslb.com/a.jpg",
                  "description": "简介", "length": "24:16", "created": 1_600_000_000_i64,
                  "mid": 12345, "author": "UP主甲" },
                { "bvid": "BV1yy" }
            ] },
            "page": { "count": 89, "pn": 1, "ps": 30 }
        });
        let r: ArcSearchResult = from_value(raw)?;
        mineral_test::assert_snap_debug!("投稿列表解析(完整项 + 缺字段项共存)", r);
        Ok(())
    }

    /// 缺 `list` 容器 → None(上层视为空页),不报错。
    #[test]
    fn missing_list_is_none() -> color_eyre::Result<()> {
        let r: ArcSearchResult = from_value(serde_json::json!({}))?;
        assert!(r.list.is_none());
        Ok(())
    }
}
