//! 歌单端点(spec §4.6 PlaylistDetail + UserPlaylist)。

use color_eyre::eyre::eyre;
use mineral_model::{
    AlbumId, AlbumRef, ArtistId, ArtistRef, Playlist, PlaylistId, Song, SongId, SourceKind, UserId,
};
use serde_json::json;

type Result<T> = color_eyre::Result<T>;

use crate::convert::parse_remote;
use crate::dto::song::AlbumSongDto;
use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;

/// 歌单内全部歌曲。
///
/// 走 `/api/v6/playlist/detail`(linuxapi);响应里 `playlist.tracks` 是 `AlbumSongDto` 形态。
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
    let dtos: Vec<AlbumSongDto> = serde_json::from_value(tracks.clone())?;

    Ok(dtos
        .into_iter()
        .map(|s| Song {
            source: SourceKind::Netease,
            id: SongId::new(s.id.to_string()),
            name: s.name,
            artists: s
                .ar
                .into_iter()
                .map(|a| ArtistRef {
                    id: ArtistId::new(a.id.to_string()),
                    name: a.name,
                })
                .collect(),
            album: Some(AlbumRef {
                id: AlbumId::new(s.al.id.to_string()),
                name: s.al.name,
            }),
            duration_ms: s.dt,
            cover_url: s.al.pic_url.as_deref().and_then(parse_remote),
            source_url: None,
        })
        .collect())
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
                source: SourceKind::Netease,
                id: PlaylistId::new(id.to_string()),
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
