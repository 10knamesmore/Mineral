//! UP 主空间端点:名片(`web-interface/card`,免签名)与投稿列表
//! (`space/wbi/arc/search`,WBI 签名 + dm_* 反爬参数)。

use crate::transport::Transport;
use crate::wire::de::from_value;
use crate::wire::space::{ArcSearchResult, CardResult};

/// 用户名片端点(免登录、免签名)。
const CARD_URL: &str = "https://api.bilibili.com/x/web-interface/card";

/// UP 主投稿列表端点。
const ARC_SEARCH_URL: &str = "https://api.bilibili.com/x/space/wbi/arc/search";

/// `dm_img_list`:鼠标轨迹采集,空数组 = 无轨迹(服务端接受)。
const DM_IMG_LIST: &str = "[]";

/// `dm_img_str`:WebGL 版本串的 base64(去 padding)。取常见真实浏览器的值
/// (`WebGL 1.0 (OpenGL ES 2.0 Chromium)`),海量真实用户同值,不构成指纹。
const DM_IMG_STR: &str = "V2ViR0wgMS4wIChPcGVuR0wgRVMgMi4wIENocm9taXVtKQ";

/// `dm_cover_img_str`:WebGL 渲染器串的 base64(去 padding),同上取常见真实值。
const DM_COVER_IMG_STR: &str =
    "QU5HTEUgKEludGVsLCBNZXNhIEludGVsKFIpIFVIRCBHcmFwaGljcyA2MjAgKEtCTCBHVDIpLCBPcGVuR0wgNC42KQ";

/// `dm_img_inter`:交互采集,空骨架 = 无交互。
const DM_IMG_INTER: &str = r#"{"ds":[],"wh":[0,0,0],"of":[0,0,0]}"#;

/// 拉用户名片(用户名 / 头像 / 签名 / 粉丝数 / 投稿数)。
///
/// # Params:
///   - `transport`: HTTP 传输层
///   - `mid`: 用户数字 ID(裸值)
///
/// # Return:
///   名片响应 DTO。
pub async fn card(transport: &Transport, mid: &str) -> color_eyre::Result<CardResult> {
    let data = transport
        .get_data(&format!("{CARD_URL}?mid={mid}&photo=false"))
        .await?;
    from_value(data)
}

/// 投稿列表的排序方式(arc/search 的 `order` 参数)。
#[derive(Debug, Clone, Copy)]
pub enum ArcOrder {
    /// 按投稿时间倒序(最新)。
    Pubdate,

    /// 按播放数倒序(最热)。
    Click,
}

impl ArcOrder {
    /// 端点 `order` 参数的取值。
    fn as_param(self) -> &'static str {
        match self {
            Self::Pubdate => "pubdate",
            Self::Click => "click",
        }
    }
}

/// 拉 UP 主投稿视频列表(页码分页)。
///
/// arc/search 是 space 域反爬最重的端点:除 WBI 签名外还校验 dm_* 采集参数,
/// 缺了服务端返 `-352`(风控)。这里按真实浏览器的形态下发一组静态值。
///
/// # Params:
///   - `transport`: HTTP 传输层
///   - `mid`: UP 主数字 ID(裸值)
///   - `order`: 排序方式(最新投稿 / 最多播放)
///   - `page`: 页码(从 1 起)
///   - `page_size`: 每页条数(服务端上限 ~50)
///
/// # Return:
///   投稿列表响应 DTO。
pub async fn arc_videos(
    transport: &Transport,
    mid: &str,
    order: ArcOrder,
    page: u32,
    page_size: u32,
) -> color_eyre::Result<ArcSearchResult> {
    let data = transport
        .get_signed(
            ARC_SEARCH_URL,
            vec![
                ("mid", mid.to_owned()),
                ("pn", page.to_string()),
                ("ps", page_size.to_string()),
                ("order", order.as_param().to_owned()),
                ("platform", "web".to_owned()),
                ("web_location", "333.1387".to_owned()),
                ("dm_img_list", DM_IMG_LIST.to_owned()),
                ("dm_img_str", DM_IMG_STR.to_owned()),
                ("dm_cover_img_str", DM_COVER_IMG_STR.to_owned()),
                ("dm_img_inter", DM_IMG_INTER.to_owned()),
            ],
        )
        .await?;
    from_value(data)
}
