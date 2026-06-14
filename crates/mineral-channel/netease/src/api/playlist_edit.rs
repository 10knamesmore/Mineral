//! 歌单写操作端点(建/删歌单、加/删歌、改名/改描述)。
//!
//! 全部统一走 weapi。参考实现里 manipulate / update-name / desc-update
//! 默认 eapi,这里走 weapi 是因为与本仓库 transport 主路径一致
//! (`/api/*` 在 weapi 网关下等价,`user_playlists` 已有先例),少一种
//! 凭据路径就少一类风控变量;若实测 weapi 版不通,降级 `Crypto::Linuxapi`,
//! 不要回 eapi(会引入 checkToken 一类的额外反作弊凭据)。
//!
//! 风控注意:512 表示风控或歌单容量满,**不要在本层自动重试**——参考实现
//! 对 512 自动重试一次的行为不抄,风控下重试只会加重,让错误冒泡给用户。

use mineral_model::{PlaylistId, SongId};
use serde_json::{Value, json};

/// 本模块内部统一的 result 别名,屏蔽 color-eyre 全名。
type Result<T> = color_eyre::Result<T>;

use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;
use crate::wire::playlist::CreatePlaylistResult;

/// 发一个 weapi 写请求,丢弃响应 body(只关心 code,非 200 已在 transport 报错)。
async fn post(
    transport: &Transport,
    path: &str,
    params: serde_json::Map<String, Value>,
) -> Result<Value> {
    transport
        .request(RequestSpec {
            path,
            crypto: Crypto::Weapi,
            params,
            ua: UaKind::Any,
        })
        .await
}

/// 创建歌单。响应自带新歌单对象(返回 wire DTO,映射归 `convert`),免一次"建完再拉列表"的往返。
pub async fn create_playlist(transport: &Transport, name: &str) -> Result<CreatePlaylistResult> {
    let raw = post(transport, "/api/playlist/create", create_params(name)).await?;
    crate::wire::de::from_value(raw)
}

/// 删除自己创建的歌单。
pub async fn delete_playlist(transport: &Transport, id: &PlaylistId) -> Result<()> {
    let mut params = serde_json::Map::new();
    // 参考实现的格式:字符串拼出来的 JSON 数组字面量
    params.insert("ids".into(), json!(format!("[{}]", id.as_str())));
    post(transport, "/api/playlist/remove", params).await?;
    Ok(())
}

/// 向歌单追加歌曲。歌曲已存在时远端返回 code 502,由 transport → channel
/// 链路透传(`Error::Api`),本层不吞。
pub async fn playlist_add_songs(
    transport: &Transport,
    id: &PlaylistId,
    songs: &[SongId],
) -> Result<()> {
    post(
        transport,
        "/api/playlist/manipulate/tracks",
        manipulate_params("add", id, songs)?,
    )
    .await?;
    Ok(())
}

/// 从歌单移除歌曲。
pub async fn playlist_remove_songs(
    transport: &Transport,
    id: &PlaylistId,
    songs: &[SongId],
) -> Result<()> {
    post(
        transport,
        "/api/playlist/manipulate/tracks",
        manipulate_params("del", id, songs)?,
    )
    .await?;
    Ok(())
}

/// 歌单改名。
pub async fn rename_playlist(transport: &Transport, id: &PlaylistId, name: &str) -> Result<()> {
    let mut params = serde_json::Map::new();
    params.insert("id".into(), json!(id.as_str()));
    params.insert("name".into(), json!(name));
    post(transport, "/api/playlist/update/name", params).await?;
    Ok(())
}

/// 修改歌单描述。
pub async fn set_playlist_description(
    transport: &Transport,
    id: &PlaylistId,
    desc: &str,
) -> Result<()> {
    let mut params = serde_json::Map::new();
    params.insert("id".into(), json!(id.as_str()));
    params.insert("desc".into(), json!(desc));
    post(transport, "/api/playlist/desc/update", params).await?;
    Ok(())
}

/// 建单请求参数(privacy=0 公开;隐私歌单与视频/共享歌单不在范围)。
fn create_params(name: &str) -> serde_json::Map<String, Value> {
    let mut params = serde_json::Map::new();
    params.insert("name".into(), json!(name));
    params.insert("privacy".into(), json!("0"));
    params.insert("type".into(), json!("NORMAL"));
    params
}

/// 加/删歌请求参数。`trackIds` 是字符串 id 的 JSON 数组再整体字符串化
/// (参考实现如此,服务端按这个格式解析)。
fn manipulate_params(
    op: &str,
    id: &PlaylistId,
    songs: &[SongId],
) -> Result<serde_json::Map<String, Value>> {
    let ids = songs
        .iter()
        .map(|s| s.as_str().to_owned())
        .collect::<Vec<String>>();
    let mut params = serde_json::Map::new();
    params.insert("op".into(), json!(op));
    params.insert("pid".into(), json!(id.as_str()));
    params.insert("trackIds".into(), json!(serde_json::to_string(&ids)?));
    params.insert("imme".into(), json!("true"));
    Ok(params)
}

#[cfg(test)]
mod tests {
    use mineral_model::{PlaylistId, SongId, SourceKind};
    use serde_json::{Value, json};

    use super::{create_params, manipulate_params};

    /// 建单参数:加密前明文必须与参考实现 byte 等价(name/privacy/type)。
    #[test]
    fn create_params_match_reference_shape() {
        let params = create_params("开车歌单");
        assert_eq!(
            Value::Object(params),
            json!({ "name": "开车歌单", "privacy": "0", "type": "NORMAL" })
        );
    }

    /// 加歌参数:trackIds 是"字符串 id 的 JSON 数组再整体字符串化",
    /// pid 用裸值(无 namespace 前缀)。
    #[test]
    fn manipulate_params_stringify_track_ids() -> color_eyre::Result<()> {
        let pl = PlaylistId::new(SourceKind::NETEASE, "123");
        let songs = vec![
            SongId::new(SourceKind::NETEASE, "186016"),
            SongId::new(SourceKind::NETEASE, "175408"),
        ];
        let params = manipulate_params("add", &pl, &songs)?;
        assert_eq!(
            Value::Object(params),
            json!({
                "op": "add", "pid": "123",
                "trackIds": "[\"186016\",\"175408\"]", "imme": "true"
            })
        );
        Ok(())
    }
}
