//! 搜索端点(纯协议:参数 → 类型化 wire DTO,DTO → model 映射归 `convert`)。

use color_eyre::eyre::eyre;
use serde::de::DeserializeOwned;
use serde_json::json;

/// 本模块内部统一的 result 别名,屏蔽 color-eyre 全名。
type Result<T> = color_eyre::Result<T>;

use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;
use crate::wire::search::{
    CloudSongsResult, SearchAlbumsResult, SearchArtistsResult, SearchPlaylistsResult,
};

/// album / artist / playlist 搜索端点,通过 `stype` 区分类别。
const PATH: &str = "/weapi/search/get";

/// 单曲走 cloudsearch:回 `ar`/`al`/`dt` 形态且带 `al.picUrl` 封面(`/weapi/search/get`
/// 的嵌套 album 只给 `picId`,封面取不到)。
const CLOUD_PATH: &str = "/weapi/cloudsearch/get/web";

/// 打搜索端点拿原始响应。`stype` 1=单曲, 10=专辑, 100=artist, 1000=歌单;`path` 选 [`PATH`]
/// 或 [`CLOUD_PATH`](两者参数同构,仅响应形态不同)。
async fn search_raw(
    transport: &Transport,
    path: &str,
    keyword: &str,
    stype: i32,
    offset: u32,
    limit: u32,
) -> Result<serde_json::Value> {
    let mut params = serde_json::Map::new();
    params.insert("s".into(), json!(keyword));
    params.insert("type".into(), json!(stype.to_string()));
    params.insert("offset".into(), json!(offset.to_string()));
    params.insert("limit".into(), json!(limit.to_string()));

    transport
        .request(RequestSpec {
            path,
            crypto: Crypto::Weapi,
            params,
            ua: UaKind::Any,
        })
        .await
}

/// 打端点拿响应、取出 `result` 子对象(各搜索端点都把命中包在 `result` 下)、反序列化成 `T`。
async fn search_typed<T: DeserializeOwned>(
    transport: &Transport,
    path: &str,
    keyword: &str,
    stype: i32,
    offset: u32,
    limit: u32,
) -> Result<T> {
    let raw = search_raw(transport, path, keyword, stype, offset, limit).await?;
    let result = raw
        .get("result")
        .ok_or_else(|| eyre!("search response missing `result`"))?;
    crate::wire::de::from_value(result.clone())
}

/// 单曲搜索(cloudsearch,`ar`/`al`/`dt` 形态,封面随 `al.picUrl` 到位)。
pub async fn search_songs(
    transport: &Transport,
    keyword: &str,
    offset: u32,
    limit: u32,
) -> Result<CloudSongsResult> {
    search_typed(transport, CLOUD_PATH, keyword, 1, offset, limit).await
}

/// 专辑搜索(只回元信息,曲目按需走 `album_detail`)。
pub async fn search_albums(
    transport: &Transport,
    keyword: &str,
    offset: u32,
    limit: u32,
) -> Result<SearchAlbumsResult> {
    search_typed(transport, PATH, keyword, 10, offset, limit).await
}

/// artist 搜索(只回元信息 + 粉丝数,简介/热门曲按需走 `artist_detail`)。
pub async fn search_artists(
    transport: &Transport,
    keyword: &str,
    offset: u32,
    limit: u32,
) -> Result<SearchArtistsResult> {
    search_typed(transport, PATH, keyword, 100, offset, limit).await
}

/// 歌单搜索(只回元信息,曲目按需走 `playlist_detail`)。
pub async fn search_playlists(
    transport: &Transport,
    keyword: &str,
    offset: u32,
    limit: u32,
) -> Result<SearchPlaylistsResult> {
    search_typed(transport, PATH, keyword, 1000, offset, limit).await
}
