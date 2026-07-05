//! 视频详情端点(`x/web-interface/view?bvid=`)的响应结构。

use serde::Deserialize;

/// 视频详情响应的 `data` 块:一个 BV 视频的完整元信息 + 分 P 列表。
#[derive(Debug, Clone, Deserialize)]
pub struct VideoInfo {
    /// 视频 BV 号。
    pub bvid: String,

    /// 视频 av 号(数字 ID)。
    #[serde(default)]
    pub aid: i64,

    /// 首 P 的 cid(单 P 视频只有这一个)。
    #[serde(default)]
    pub cid: i64,

    /// 视频标题。
    pub title: String,

    /// 简介(可缺)。
    #[serde(default)]
    pub desc: Option<String>,

    /// 封面 URL(可缺;可能协议相对)。
    #[serde(default)]
    pub pic: Option<String>,

    /// 总时长(秒;多 P 视频为各 P 之和,可缺)。
    #[serde(default)]
    pub duration: Option<i64>,

    /// 发布时间(Unix epoch 秒,可缺)。
    #[serde(default)]
    pub pubdate: Option<i64>,

    /// UP 主信息。
    pub owner: VideoOwner,

    /// 分 P 列表(可缺;单 P 视频有时也带一项)。
    #[serde(default)]
    pub pages: Option<Vec<VideoPage>>,
}

/// 视频 UP 主。
#[derive(Debug, Clone, Deserialize)]
pub struct VideoOwner {
    /// UP 主数字 ID。
    pub mid: i64,

    /// UP 主名。
    pub name: String,

    /// UP 主头像 URL。
    #[serde(default)]
    pub face: String,
}

/// 视频里的一个分 P。
#[derive(Debug, Clone, Deserialize)]
pub struct VideoPage {
    /// 该 P 的 cid(取流 / 播放定位用)。
    pub cid: i64,

    /// 分 P 序号(从 1 起)。
    pub page: i32,

    /// 分 P 标题。
    pub part: String,

    /// 该 P 时长(秒)。
    #[serde(default)]
    pub duration: i64,
}

#[cfg(test)]
mod tests {
    use super::VideoInfo;
    use crate::wire::de::from_value;

    /// 正常解析多 P 视频详情:owner / pages / 各字段到位。
    #[test]
    fn parses_multi_page_video() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "bvid": "BV1xx411c7mD", "aid": 800, "cid": 1001,
            "title": "多P合集", "desc": "简介",
            "pic": "//i0.hdslb.com/cover.jpg", "duration": 480, "pubdate": 1_600_000_000_i64,
            "owner": { "mid": 12345, "name": "UP主甲", "face": "//i0.hdslb.com/face.jpg" },
            "pages": [
                { "cid": 1001, "page": 1, "part": "第一话", "duration": 240 },
                { "cid": 1002, "page": 2, "part": "第二话", "duration": 240 }
            ]
        });
        let info: VideoInfo = from_value(raw)?;
        mineral_test::assert_snap_debug!("视频详情解析(2P 合集 + owner + pages)", info);
        Ok(())
    }

    /// 单 P 视频:`pages` 缺失应落 `None` 而非报错。
    #[test]
    fn tolerates_missing_pages() -> color_eyre::Result<()> {
        let raw = serde_json::json!({
            "bvid": "BV1yy", "title": "单P", "duration": 120,
            "owner": { "mid": 1, "name": "乙" }
        });
        let info: VideoInfo = from_value(raw)?;
        assert!(info.pages.is_none(), "pages 缺失应解析成 None");
        assert_eq!(info.owner.face, "", "face 缺失应落空串");
        Ok(())
    }
}
