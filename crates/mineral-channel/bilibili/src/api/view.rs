//! 视频详情端点(`web-interface/view`,免登录、免签名)。

use crate::transport::Transport;
use crate::wire::de::from_value;
use crate::wire::view::VideoInfo;

/// 视频详情端点。
const VIEW_URL: &str = "https://api.bilibili.com/x/web-interface/view";

/// 拉一个 BV 视频的详情(含分 P 列表 + 各 P cid)。
///
/// # Params:
///   - `transport`: HTTP 传输层
///   - `bvid`: 视频 BV 号
///
/// # Return:
///   视频详情 DTO。
pub async fn video_info(transport: &Transport, bvid: &str) -> color_eyre::Result<VideoInfo> {
    let data = transport
        .get_data(&format!("{VIEW_URL}?bvid={bvid}"))
        .await?;
    from_value(data)
}
