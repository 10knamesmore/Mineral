//! 歌手端点(详情 / 粉丝数 / 专辑列表;纯协议:参数 → 类型化 wire DTO)。
//!
//! 详情端点顶层不带粉丝数,粉丝数只有 follow/count 端点给——两者各是独立纯端点,
//! "并发取 + 聚合成完整 Artist"的编排在 channel 层,本层只暴露单端点。

use mineral_model::ArtistId;

/// 本模块内部统一的 result 别名,屏蔽 color-eyre 全名。
type Result<T> = color_eyre::Result<T>;

use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;
use crate::wire::artist::{ArtistAlbumsResult, ArtistDetailResult, FollowCountResult};

/// 歌手详情:`/weapi/v1/artist/{id}`(简介 / 计数 / 热门曲)。
///
/// 响应顶层平铺(`artist` / `hotSongs` 与 `code` 同级,无 `result` 包裹)。
pub async fn detail(transport: &Transport, id: &ArtistId) -> Result<ArtistDetailResult> {
    let path = format!("/weapi/v1/artist/{}", id.as_str());
    let raw = transport
        .request(RequestSpec {
            path: &path,
            crypto: Crypto::Weapi,
            params: serde_json::Map::new(),
            ua: UaKind::Any,
        })
        .await?;
    crate::wire::de::from_value(raw)
}

/// 歌手粉丝数:`/api/artist/follow/count/get`(取 `data.fansCnt`;`data` 缺降级 0)。
pub async fn follow_count(transport: &Transport, id: &ArtistId) -> Result<u64> {
    let mut params = serde_json::Map::new();
    params.insert("id".into(), serde_json::json!(id.as_str()));
    let raw = transport
        .request(RequestSpec {
            path: "/api/artist/follow/count/get",
            crypto: Crypto::Weapi,
            params,
            ua: UaKind::Any,
        })
        .await?;
    let parsed: FollowCountResult = crate::wire::de::from_value(raw)?;
    Ok(parsed.data.map_or(0, |d| d.fans_cnt))
}

/// 歌手专辑列表:`/api/artist/albums/{id}`(分页;曲目留空,按需走 `album_detail`)。
pub async fn albums(
    transport: &Transport,
    id: &ArtistId,
    offset: u32,
    limit: u32,
) -> Result<ArtistAlbumsResult> {
    let path = format!("/api/artist/albums/{}", id.as_str());
    let mut params = serde_json::Map::new();
    params.insert("limit".into(), serde_json::json!(limit.to_string()));
    params.insert("offset".into(), serde_json::json!(offset.to_string()));
    params.insert("total".into(), serde_json::json!("true"));
    let raw = transport
        .request(RequestSpec {
            path: &path,
            crypto: Crypto::Weapi,
            params,
            ua: UaKind::Any,
        })
        .await?;
    crate::wire::de::from_value(raw)
}
