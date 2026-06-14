//! 歌单端点(详情 + 用户歌单列表)。

use mineral_model::{PlaylistId, UserId};
use serde_json::json;

/// 本模块内部统一的 result 别名,屏蔽 color-eyre 全名。
type Result<T> = color_eyre::Result<T>;

use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;
use crate::wire::playlist::{PlaylistDetailResult, UserPlaylistsResult};

/// 歌单详情端点 `/api/v6/playlist/detail`(linuxapi)。纯端点——只打端点 + 解析成 DTO。
///
/// `limit`/`n` 控制返回的 tracks 量:`0` = 只要元信息 + `trackIds` 顺序(不拉 tracks,
/// 用于缓存版本比对);大值 = 连 tracks 全拉。元信息(name/简介/封面/计数/版本戳)两种
/// limit 都返回。
pub async fn detail(
    transport: &Transport,
    id: &PlaylistId,
    limit: u32,
) -> Result<PlaylistDetailResult> {
    let mut p = serde_json::Map::new();
    p.insert("id".into(), json!(id.as_str()));
    p.insert("offset".into(), json!("0"));
    p.insert("total".into(), json!("true"));
    p.insert("limit".into(), json!(limit.to_string()));
    p.insert("n".into(), json!(limit.to_string()));
    let v = transport
        .request(RequestSpec {
            path: "/api/v6/playlist/detail",
            crypto: Crypto::Linuxapi,
            params: p,
            ua: UaKind::Linux,
        })
        .await?;
    crate::wire::de::from_value(v)
}

/// 用户歌单列表端点 `/weapi/user/playlist`。纯端点。
pub async fn user_playlists(transport: &Transport, uid: &UserId) -> Result<UserPlaylistsResult> {
    let mut p = serde_json::Map::new();
    p.insert("uid".into(), json!(uid.as_str()));
    p.insert("offset".into(), json!("0"));
    p.insert("limit".into(), json!("1000"));
    let v = transport
        .request(RequestSpec {
            path: "/weapi/user/playlist",
            crypto: Crypto::Weapi,
            params: p,
            ua: UaKind::Pc,
        })
        .await?;
    crate::wire::de::from_value(v)
}
