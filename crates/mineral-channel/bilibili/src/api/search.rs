//! 视频搜索端点(`wbi/search/type`,WBI 签名 + 需 buvid3)。

use mineral_model::Song;

use crate::convert::search_video_to_song;
use crate::transport::Transport;
use crate::wire::de::from_value;
use crate::wire::search::SearchResult;

/// 视频搜索端点。
const SEARCH_URL: &str = "https://api.bilibili.com/x/web-interface/wbi/search/type";

/// 搜视频 → `Vec<Song>`(每命中项以其 P1 代表;缺 bvid 的项被 convert 丢弃)。
///
/// # Params:
///   - `transport`: HTTP 传输层
///   - `keyword`: 搜索关键词
///   - `page`: 页码(从 1 起)
///
/// # Return:
///   命中视频映射成的单曲列表。
pub async fn search_songs(
    transport: &Transport,
    keyword: &str,
    page: u32,
) -> color_eyre::Result<Vec<Song>> {
    let data = transport
        .get_signed(
            SEARCH_URL,
            vec![
                ("search_type", "video".to_owned()),
                ("keyword", keyword.to_owned()),
                ("page", page.to_string()),
                ("order", "totalrank".to_owned()),
            ],
        )
        .await?;
    let result: SearchResult = from_value(data)?;
    Ok(result
        .result
        .into_iter()
        .filter_map(search_video_to_song)
        .collect())
}
