//! 专辑端点。

use mineral_model::AlbumId;

/// 本模块内部统一的 result 别名,屏蔽 color-eyre 全名。
type Result<T> = color_eyre::Result<T>;

use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;
use crate::wire::search::AlbumDetailResult;

/// 专辑详情端点 `/weapi/v1/album/{id}`:一次返回顶层元信息(简介 / 发行信息 / 曲目数)
/// 与曲目列表。纯端点——只打端点 + 解析成类型化 DTO,取舍 / 映射 model 交给上层。
pub async fn detail(transport: &Transport, album_id: &AlbumId) -> Result<AlbumDetailResult> {
    let path = format!("/weapi/v1/album/{}", album_id.as_str());
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
