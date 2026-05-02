//! 当前登录用户相关的端点。
//!
//! 目前只暴露一个能力:从 cookie jar 出发拿到登录用户的 `userId`。
//! 这是后续 `user_playlists` 必需的入参,把它独立抽出来,避免上层重复手抠 JSON。

use std::collections::HashSet;

use color_eyre::eyre::eyre;
use mineral_model::{SongId, UserId};
use serde_json::json;

type Result<T> = color_eyre::Result<T>;

use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;
use crate::wire::user::LikeListResp;

/// 拉取当前登录用户的 `userId`。
///
/// # Params:
///   - `transport`: 已注入登录 cookie 的 [`Transport`]
///
/// # Return:
///   登录用户的 [`UserId`](mineral_model::UserId);未登录或 cookie 失效时返回 `Err`。
pub async fn account_uid(transport: &Transport) -> Result<UserId> {
    let v = transport
        .request(RequestSpec {
            path: "/api/nuser/account/get",
            crypto: Crypto::Weapi,
            params: serde_json::Map::new(),
            ua: UaKind::Pc,
        })
        .await?;
    let id = v
        .get("account")
        .and_then(|x| x.get("id"))
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| eyre!("account response missing `account.id` (logged in?)"))?;
    Ok(UserId::new(id.to_string()))
}

/// 当前用户喜欢(♥)的歌曲 ID 集合。
///
/// 走 `/weapi/song/like/get`(`/likelist` 别名),crypto = weapi。
/// 响应 `{ids: [number, ...]}`,数字转成 [`SongId`] 字符串。
pub async fn liked_song_ids(transport: &Transport, uid: &UserId) -> Result<HashSet<SongId>> {
    let mut p = serde_json::Map::new();
    p.insert("uid".into(), json!(uid.as_str()));

    let v = transport
        .request(RequestSpec {
            path: "/weapi/song/like/get",
            crypto: Crypto::Weapi,
            params: p,
            ua: UaKind::Pc,
        })
        .await?;
    let resp: LikeListResp = serde_json::from_value(v)?;
    Ok(resp
        .ids
        .into_iter()
        .map(|n| SongId::new(n.to_string()))
        .collect())
}
