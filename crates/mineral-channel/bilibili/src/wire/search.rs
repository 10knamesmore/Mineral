//! 搜索端点(`x/web-interface/wbi/search/type?search_type=video`)的响应结构。

use serde::Deserialize;

/// 搜索响应的 `data` 块:`result` 数组即命中的视频列表。
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResult {
    /// 命中的视频条目(无命中 / 字段缺失 → 空)。
    #[serde(default)]
    pub result: Vec<SearchVideoItem>,

    /// 总页数(服务端按其页大小折算,上限 50 页)。「还有没有下一页」以它为准——
    /// B站每页实际条数由服务端决定,靠条数推断会误判榨干。缺失落 `None`(上层回退推断)。
    #[serde(default, rename = "numPages")]
    pub num_pages: Option<u32>,
}

/// 搜索结果里的单个视频条目(`search_type=video`)。
///
/// B站搜索项字段普遍可缺,故全部 `Option` + `#[serde(default)]`——宁可让缺字段的项
/// 在 convert 处被判为不可用(返回 `None`)也不让整批反序列化炸掉。
#[derive(Debug, Clone, Deserialize)]
pub struct SearchVideoItem {
    /// 视频 BV 号(如 `BV1xx411c7mD`);缺失则该项无法定位,convert 会丢弃。
    #[serde(default)]
    pub bvid: Option<String>,

    /// 标题。B站在此字段里塞 `<em class="keyword">...</em>` 高亮标签,convert 需 strip。
    #[serde(default)]
    pub title: Option<String>,

    /// UP 主名。
    #[serde(default)]
    pub author: Option<String>,

    /// UP 主数字 ID。
    #[serde(default)]
    pub mid: Option<i64>,

    /// 封面 URL。可能以 `//` 开头(协议相对),convert 需补 `https:`。
    #[serde(default)]
    pub pic: Option<String>,

    /// 时长,`"mm:ss"` 文本格式,convert 需解析成毫秒。
    #[serde(default)]
    pub duration: Option<String>,

    /// 播放量。
    #[serde(default)]
    pub play: Option<i64>,

    /// 简介。
    #[serde(default)]
    pub description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::SearchResult;
    use crate::wire::de::from_value;

    /// 正常解析搜索响应:含高亮标签的标题、`mm:ss` 时长、协议相对封面原样进 DTO
    /// (清洗留给 convert),分页元信息 numPages 一并解析。
    #[test]
    fn parses_search_result() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "numPages": 34,
            "result": [
                { "bvid": "BV1xx411c7mD",
                  "title": "【<em class=\"keyword\">初音</em>】曲名",
                  "author": "UP主甲", "mid": 12345,
                  "pic": "//i0.hdslb.com/x.jpg", "duration": "3:45",
                  "play": 99_999, "description": "简介" },
                // 字段大面积缺失:仍应解析成功,余字段落 None。
                { "bvid": "BV1yy" }
            ]
        });
        let r: SearchResult = from_value(raw)?;
        mineral_test::assert_snap_debug!(
            "搜索响应解析(高亮标题 / mm:ss 时长 / 协议相对封面 + 缺字段项)",
            r
        );
        Ok(())
    }

    /// 缺 `result` 字段(无命中)→ 空列表,缺 `numPages` → None,都不报错。
    #[test]
    fn missing_result_field_is_empty() -> color_eyre::Result<()> {
        let r: SearchResult = from_value(serde_json::json!({}))?;
        assert!(r.result.is_empty());
        assert_eq!(r.num_pages, None, "缺分页元信息回退 None");
        Ok(())
    }
}
