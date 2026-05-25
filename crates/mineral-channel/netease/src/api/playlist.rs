//! 歌单端点(spec §4.6 PlaylistDetail + UserPlaylist)。

use color_eyre::eyre::eyre;
use mineral_model::{
    AlbumId, AlbumRef, ArtistId, ArtistRef, Playlist, PlaylistId, Song, SongId, SourceKind, UserId,
};
use serde_json::json;

/// 本模块内部统一的 result 别名,屏蔽 color-eyre 全名。
type Result<T> = color_eyre::Result<T>;

use crate::convert::parse_remote;
use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;
use crate::wire::song::AlbumSong;

/// 歌单内全部歌曲。
///
/// 走 `/api/v6/playlist/detail`(linuxapi);响应里 `playlist.tracks` 是 `wire::song::AlbumSong` 形态。
pub async fn songs_in_playlist(transport: &Transport, id: &PlaylistId) -> Result<Vec<Song>> {
    let mut p = serde_json::Map::new();
    p.insert("id".into(), json!(id.as_str()));
    p.insert("offset".into(), json!("0"));
    p.insert("total".into(), json!("true"));
    p.insert("limit".into(), json!("1000"));
    p.insert("n".into(), json!("1000"));

    let v = transport
        .request(RequestSpec {
            path: "/api/v6/playlist/detail",
            crypto: Crypto::Linuxapi,
            params: p,
            ua: UaKind::Linux,
        })
        .await?;
    let tracks = v
        .get("playlist")
        .and_then(|x| x.get("tracks"))
        .ok_or_else(|| eyre!("playlist response missing playlist.tracks"))?;
    let dtos: Vec<AlbumSong> = crate::wire::de::from_value(tracks.clone())?;

    Ok(dtos
        .into_iter()
        .map(|s| Song {
            id: SongId::new(SourceKind::NETEASE, s.id.to_string()),
            name: s.name,
            artists: s
                .ar
                .into_iter()
                .map(|a| ArtistRef {
                    id: ArtistId::new(SourceKind::NETEASE, a.id.to_string()),
                    name: a.name,
                })
                .collect(),
            album: Some(AlbumRef {
                id: AlbumId::new(SourceKind::NETEASE, s.al.id.to_string()),
                name: s.al.name,
            }),
            duration_ms: s.dt,
            cover_url: s.al.pic_url.as_deref().and_then(parse_remote),
            source_url: None,
        })
        .collect())
}

/// 轻量拉歌单版本信息(`trackUpdateTime` + 全量 `trackIds` 顺序),不拉完整 tracks。
///
/// 用于缓存条件刷新的版本比对:先轻量拿版本戳和曲目 id 顺序,变了才全拉。
///
/// **假设**:`limit=0, n=0` 时网易云只返回元信息 + 全量 `trackIds`(不受 limit
/// 控制),省掉 `tracks` 大头(受 limit 控制)。若实际仍返回 tracks,则本函数只是
/// 没省到带宽,功能不受影响——版本比对仍走 `trackUpdateTime`,顺序仍走 `trackIds`,
/// 必要时可回退到比对 `trackIds` 而非依赖省带宽。真机需验证 limit=0 是否真省 tracks。
///
/// # Params:
///   - `transport`: 网易云请求通道
///   - `id`: 歌单 id
///
/// # Return:
///   `(track_update_time, track_ids)`:版本戳(unix ms)与全量曲目裸值(保序)。
pub async fn playlist_version(
    transport: &Transport,
    id: &PlaylistId,
) -> Result<(i64, Vec<String>)> {
    let mut p = serde_json::Map::new();
    p.insert("id".into(), json!(id.as_str()));
    p.insert("offset".into(), json!("0"));
    p.insert("total".into(), json!("true"));
    p.insert("limit".into(), json!("0"));
    p.insert("n".into(), json!("0"));

    let v = transport
        .request(RequestSpec {
            path: "/api/v6/playlist/detail",
            crypto: Crypto::Linuxapi,
            params: p,
            ua: UaKind::Linux,
        })
        .await?;
    let playlist = v
        .get("playlist")
        .ok_or_else(|| eyre!("playlist_version response missing `playlist`"))?;

    let track_update_time = playlist
        .get("trackUpdateTime")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| eyre!("playlist_version missing playlist.trackUpdateTime"))?;

    let track_ids = playlist
        .get("trackIds")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| eyre!("playlist_version missing playlist.trackIds"))?
        .iter()
        .map(|it| {
            it.get("id")
                .and_then(serde_json::Value::as_i64)
                .map(|n| n.to_string())
                .ok_or_else(|| eyre!("playlist_version trackIds entry missing numeric `id`"))
        })
        .collect::<Result<Vec<String>>>()?;

    Ok((track_update_time, track_ids))
}

/// 用户歌单列表。
pub async fn user_playlists(transport: &Transport, uid: &UserId) -> Result<Vec<Playlist>> {
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
    let arr = v
        .get("playlist")
        .ok_or_else(|| eyre!("user_playlists missing `playlist` array"))?;

    let mut out = Vec::new();
    if let Some(items) = arr.as_array() {
        for it in items {
            let id = it
                .get("id")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let name = it
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let description = it
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let cover_url = it
                .get("coverImgUrl")
                .and_then(serde_json::Value::as_str)
                .and_then(parse_remote);
            let track_count = it
                .get("trackCount")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default();
            out.push(Playlist {
                id: PlaylistId::new(SourceKind::NETEASE, id.to_string()),
                name,
                description,
                cover_url,
                track_count,
                songs: Vec::new(),
            });
        }
    }
    Ok(out)
}
