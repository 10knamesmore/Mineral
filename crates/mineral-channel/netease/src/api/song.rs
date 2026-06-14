//! 歌曲详情、播放 URL、红心、回忆坐标端点(纯协议:参数 → 类型化 wire DTO)。
//!
//! 播放 URL 有两个网易端点:v1(`/weapi/song/enhance/player/url/v1`,字符串等级)与
//! legacy(`/api/song/enhance/player/url`,数字 br)。两者各是纯端点、返回 [`SongUrl`] 列表;
//! spec §4.3 的"v1 失败 / 仅试听 → 降级 legacy"双层降级编排在 channel 层,不在本层。

use color_eyre::eyre::eyre;
use mineral_model::{BitRate, SongId};
use serde_json::json;

/// 本模块内部统一的 result 别名,屏蔽 color-eyre 全名。
type Result<T> = color_eyre::Result<T>;

use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;
use crate::wire::song::{AlbumSong, FirstListenInfo, SongUrl};

/// 详情:`/weapi/v3/song/detail`。返回 `ar`/`al`/`dt` 形态的 wire DTO(映射归 `convert`）。
pub async fn songs_detail(transport: &Transport, ids: &[SongId]) -> Result<Vec<AlbumSong>> {
    let c: Vec<serde_json::Value> = ids.iter().map(|i| json!({ "id": i.as_str() })).collect();
    let mut p = serde_json::Map::new();
    p.insert("c".into(), json!(serde_json::to_string(&c)?));

    let v = transport
        .request(RequestSpec {
            path: "/weapi/v3/song/detail",
            crypto: Crypto::Weapi,
            params: p,
            ua: UaKind::Any,
        })
        .await?;
    let songs = v
        .get("songs")
        .ok_or_else(|| eyre!("songs_detail response missing `songs`"))?;
    crate::wire::de::from_value(songs.clone())
}

/// 播放 URL · v1(`/weapi/song/enhance/player/url/v1`,字符串等级)。返回 wire DTO 列表。
pub async fn song_url_v1(
    transport: &Transport,
    ids: &[SongId],
    quality: BitRate,
) -> Result<Vec<SongUrl>> {
    let id_list: Vec<&str> = ids.iter().map(SongId::as_str).collect();
    let level = match quality {
        BitRate::Standard => "standard",
        BitRate::Higher => "higher",
        BitRate::Exhigh => "exhigh",
        BitRate::Lossless => "lossless",
        BitRate::Hires => "hires",
    };

    let mut p = serde_json::Map::new();
    p.insert("ids".into(), json!(serde_json::to_string(&id_list)?));
    p.insert("level".into(), json!(level));
    p.insert("encodeType".into(), json!("flac"));

    let v = transport
        .request(RequestSpec {
            path: "/weapi/song/enhance/player/url/v1",
            crypto: Crypto::Weapi,
            params: p,
            ua: UaKind::Any,
        })
        .await?;
    parse_song_url_dtos(&v)
}

/// 播放 URL · legacy(`/api/song/enhance/player/url`,linuxapi,数字 br)。返回 wire DTO 列表。
pub async fn song_url_legacy(
    transport: &Transport,
    ids: &[SongId],
    quality: BitRate,
) -> Result<Vec<SongUrl>> {
    let id_list: Vec<i64> = ids.iter().filter_map(|i| i.as_str().parse().ok()).collect();
    let br = match quality {
        BitRate::Standard => "128000",
        BitRate::Higher => "192000",
        BitRate::Exhigh => "320000",
        BitRate::Lossless => "999000",
        BitRate::Hires => "1900000",
    };

    let mut p = serde_json::Map::new();
    p.insert("ids".into(), json!(serde_json::to_string(&id_list)?));
    p.insert("br".into(), json!(br));

    let v = transport
        .request(RequestSpec {
            path: "/api/song/enhance/player/url",
            crypto: Crypto::Linuxapi,
            params: p,
            ua: UaKind::Linux,
        })
        .await?;
    parse_song_url_dtos(&v)
}

/// 组装 `/api/radio/like` 请求所需的 params。
///
/// 独立提取为纯函数,方便单元测试在不实发请求的情况下验证字段正确性。
///
/// # Params:
///   - `id`: 目标歌曲,取裸值作为 `trackId`
///   - `like`: `true` → `"true"`,`false` → `"false"`
///
/// # Return:
///   包含 `trackId`、`like`、`alg`、`time` 四个键的 [`serde_json::Map`]。
fn build_like_params(id: &SongId, like: bool) -> serde_json::Map<String, serde_json::Value> {
    let mut p = serde_json::Map::new();
    p.insert("trackId".into(), json!(id.as_str()));
    p.insert("like".into(), json!(if like { "true" } else { "false" }));
    p.insert("alg".into(), json!("itembased"));
    p.insert("time".into(), json!("3"));
    p
}

/// 红心 / 取消红心一首歌(网易云 `/api/radio/like`)。
///
/// # Params:
///   - `transport`: 网易云请求通道
///   - `id`: 目标歌曲
///   - `like`: `true` 红心、`false` 取消
///
/// # Return:
///   成功返回 `Ok(())`;接口 code≠200 或网络错误返回 `Err`。
pub async fn like_song(transport: &Transport, id: &SongId, like: bool) -> Result<()> {
    let p = build_like_params(id, like);
    transport
        .request(RequestSpec {
            path: "/api/radio/like",
            crypto: Crypto::Weapi,
            params: p,
            ua: UaKind::Pc,
        })
        .await?;
    Ok(())
}

/// 拉取当前用户对一首歌的真实累计播放次数(回忆坐标 eapi 端点)。
///
/// 走 `/api/content/activity/music/first/listen/info`,eapi 加密,需登录态(MUSIC_U)。
/// 响应取 `data.musicTotalPlayDto.playCount`;无记录 / 字段缺失计 0。
///
/// # Params:
///   - `transport`: 网易云请求通道(须带登录 cookie)
///   - `id`: 目标歌曲,取裸值作 `songId`
///
/// # Return:
///   累计播放次数;接口 code≠200 或网络错误返回 `Err`。
pub async fn remote_play_count(transport: &Transport, id: &SongId) -> Result<u32> {
    let mut p = serde_json::Map::new();
    p.insert("songId".into(), json!(id.as_str()));
    let v = transport
        .request(RequestSpec {
            path: "/api/content/activity/music/first/listen/info",
            crypto: Crypto::Eapi,
            params: p,
            ua: UaKind::Pc,
        })
        .await?;
    let info: FirstListenInfo = crate::wire::de::from_value(v)?;
    Ok(info
        .data
        .and_then(|d| d.music_total_play)
        .map(|m| m.play_count)
        .unwrap_or_default())
}

/// 取 v1 / legacy 两套响应里共有的 `data: [...]` 反序列化成 [`SongUrl`] 列表(两端点同构)。
fn parse_song_url_dtos(v: &serde_json::Value) -> Result<Vec<SongUrl>> {
    let data = v
        .get("data")
        .ok_or_else(|| eyre!("song url response missing `data`"))?;
    crate::wire::de::from_value(data.clone())
}

#[cfg(test)]
mod tests {
    use mineral_model::{SongId, SourceKind};

    use super::build_like_params;

    /// 验证 `build_like_params` 在 `like=true` 时组装出正确的四个字段。
    #[test]
    fn like_params_true() -> color_eyre::Result<()> {
        let id = SongId::new(SourceKind::NETEASE, "123456".to_owned());
        let p = build_like_params(&id, /*like*/ true);

        assert_eq!(p.get("trackId").and_then(|v| v.as_str()), Some("123456"));
        assert_eq!(p.get("like").and_then(|v| v.as_str()), Some("true"));
        assert_eq!(p.get("alg").and_then(|v| v.as_str()), Some("itembased"));
        assert_eq!(p.get("time").and_then(|v| v.as_str()), Some("3"));
        Ok(())
    }

    /// 验证 `build_like_params` 在 `like=false` 时 `like` 字段为 `"false"`。
    #[test]
    fn like_params_false() -> color_eyre::Result<()> {
        let id = SongId::new(SourceKind::NETEASE, "654321".to_owned());
        let p = build_like_params(&id, /*like*/ false);

        assert_eq!(p.get("trackId").and_then(|v| v.as_str()), Some("654321"));
        assert_eq!(p.get("like").and_then(|v| v.as_str()), Some("false"));
        assert_eq!(p.get("alg").and_then(|v| v.as_str()), Some("itembased"));
        assert_eq!(p.get("time").and_then(|v| v.as_str()), Some("3"));
        Ok(())
    }
}
