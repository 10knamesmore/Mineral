//! 专辑端点。

use mineral_model::AlbumId;
use serde_json::{Map, Value};

use crate::Result;
use crate::request::RequestPolicy;

use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;
use crate::wire::album::SavedAlbumsResult;
use crate::wire::search::AlbumDetailResult;

/// 取当前 cookie 所属账号收藏的专辑页，不需要显式用户 ID。
pub async fn saved(
    transport: &Transport,
    offset: usize,
    limit: usize,
) -> Result<SavedAlbumsResult> {
    let mut params = Map::new();
    params.insert("offset".to_owned(), Value::from(offset));
    params.insert("limit".to_owned(), Value::from(limit));
    params.insert("total".to_owned(), Value::Bool(true));
    let raw = transport
        .request(RequestSpec {
            path: "/weapi/album/sublist",
            crypto: Crypto::Weapi,
            params,
            ua: UaKind::Any,
        })
        .await?;
    crate::wire::de::from_value(raw)
}

/// 专辑详情端点 `/weapi/v1/album/{id}`:一次返回顶层元信息(简介 / 发行信息 / 曲目数)
/// 与曲目列表。纯端点——只打端点 + 解析成类型化 DTO,取舍 / 映射 model 交给上层。
pub(crate) async fn detail(
    transport: &Transport,
    policy: &RequestPolicy,
    album_id: &AlbumId,
) -> Result<AlbumDetailResult> {
    let path = format!("/weapi/v1/album/{}", album_id.as_str());
    let raw = transport
        .request_with_policy(
            RequestSpec {
                path: &path,
                crypto: Crypto::Weapi,
                params: serde_json::Map::new(),
                ua: UaKind::Any,
            },
            policy,
        )
        .await?;
    crate::wire::de::from_value(raw)
}
