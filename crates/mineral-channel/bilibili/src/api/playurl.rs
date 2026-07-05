//! 播放地址端点(`player/wbi/playurl`,WBI 签名)。

use crate::transport::Transport;
use crate::wire::de::from_value;
use crate::wire::playurl::PlayUrlResult;

/// 播放地址端点。
const PLAYURL_URL: &str = "https://api.bilibili.com/x/player/wbi/playurl";

/// 取某分 P 的 DASH 播放信息(`fnval=4048` 请求 DASH,`fourk=1` 放开 4K/高码率档)。
///
/// # Params:
///   - `transport`: HTTP 传输层
///   - `bvid`: 视频 BV 号
///   - `cid`: 分 P 的 cid(由 view 端点取得)
///
/// # Return:
///   playurl DTO(含 `dash.audio[]`)。
pub async fn playurl(
    transport: &Transport,
    bvid: &str,
    cid: i64,
) -> color_eyre::Result<PlayUrlResult> {
    let data = transport
        .get_signed(
            PLAYURL_URL,
            vec![
                ("bvid", bvid.to_owned()),
                ("cid", cid.to_string()),
                ("fnval", "4048".to_owned()),
                ("fourk", "1".to_owned()),
            ],
        )
        .await?;
    from_value(data)
}
