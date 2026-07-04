//! 视频搜索端点(`wbi/search/type`,WBI 签名 + 需 buvid3)。

use mineral_channel_core::SearchHits;
use mineral_model::Song;

use crate::convert::search_video_to_song;
use crate::transport::Transport;
use crate::wire::de::from_value;
use crate::wire::search::SearchResult;

/// 视频搜索端点。
const SEARCH_URL: &str = "https://api.bilibili.com/x/web-interface/wbi/search/type";

/// 搜视频 → 一页 `Song` 命中(每命中项以其 P1 代表;缺 bvid 的项被 convert 丢弃)。
///
/// `page_size` 显式下发(上限 50,不发则服务端默认 ~20);「还有下一页吗」从响应的
/// `numPages` 算出——本页条数会被 convert 过滤缩水,不能当翻页依据。
///
/// # Params:
///   - `transport`: HTTP 传输层
///   - `keyword`: 搜索关键词
///   - `page`: 页码(从 1 起)
///   - `page_size`: 每页条数
///
/// # Return:
///   命中视频映射成的单曲页(带显式 `has_more`)。
pub async fn search_songs(
    transport: &Transport,
    keyword: &str,
    page: u32,
    page_size: u32,
) -> color_eyre::Result<SearchHits<Song>> {
    let data = transport
        .get_signed(
            SEARCH_URL,
            vec![
                ("search_type", "video".to_owned()),
                ("keyword", keyword.to_owned()),
                ("page", page.to_string()),
                ("page_size", page_size.to_string()),
                ("order", "totalrank".to_owned()),
            ],
        )
        .await?;
    let result: SearchResult = from_value(data)?;
    let items = result
        .result
        .into_iter()
        .filter_map(search_video_to_song)
        .collect::<Vec<Song>>();
    Ok(SearchHits {
        items,
        has_more: result.num_pages.map(|total| page < total),
    })
}
