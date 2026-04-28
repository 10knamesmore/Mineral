//! 搜索端点。

use anyhow::{anyhow, Result};
use mineral_model::{
    Album, AlbumId, AlbumRef, ArtistId, ArtistRef, Playlist, PlaylistId, Song, SongId, SourceKind,
};
use serde_json::json;

use crate::convert::parse_remote;

use crate::dto::search::{SearchAlbumsResult, SearchPlaylistsResult, SearchSongsResult};
use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;

const PATH: &str = "/weapi/search/get";

/// 搜索单曲。`stype` 1=单曲, 10=专辑, 1000=歌单。
async fn search_raw(
    transport: &Transport,
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
            path: PATH,
            crypto: Crypto::Weapi,
            params,
            ua: UaKind::Any,
        })
        .await
}

pub async fn search_songs(
    transport: &Transport,
    keyword: &str,
    offset: u32,
    limit: u32,
) -> Result<Vec<Song>> {
    let raw = search_raw(transport, keyword, 1, offset, limit).await?;
    let result = raw
        .get("result")
        .ok_or_else(|| anyhow!("search response missing `result`"))?;
    let parsed: SearchSongsResult = serde_json::from_value(result.clone())?;
    Ok(parsed
        .songs
        .into_iter()
        .map(|s| Song {
            source: SourceKind::Netease,
            id: SongId::new(s.id.to_string()),
            name: s.name,
            artists: s
                .artists
                .into_iter()
                .map(|a| ArtistRef {
                    id: ArtistId::new(a.id.to_string()),
                    name: a.name,
                })
                .collect(),
            album: Some(AlbumRef {
                id: AlbumId::new(s.album.id.to_string()),
                name: s.album.name,
            }),
            duration_ms: s.duration,
            cover_url: s.album.pic_url.as_deref().and_then(parse_remote),
            source_url: None,
        })
        .collect())
}

pub async fn search_albums(
    transport: &Transport,
    keyword: &str,
    offset: u32,
    limit: u32,
) -> Result<Vec<Album>> {
    let raw = search_raw(transport, keyword, 10, offset, limit).await?;
    let result = raw
        .get("result")
        .ok_or_else(|| anyhow!("search response missing `result`"))?;
    let parsed: SearchAlbumsResult = serde_json::from_value(result.clone())?;
    Ok(parsed
        .albums
        .into_iter()
        .map(|a| Album {
            source: SourceKind::Netease,
            id: AlbumId::new(a.id.to_string()),
            name: a.name,
            artists: a
                .artist
                .map(|x| {
                    vec![ArtistRef {
                        id: ArtistId::new(x.id.to_string()),
                        name: x.name,
                    }]
                })
                .unwrap_or_default(),
            description: a.description,
            publish_time_ms: a.publish_time,
            cover_url: a.pic_url.as_deref().and_then(parse_remote),
            songs: Vec::new(),
        })
        .collect())
}

pub async fn search_playlists(
    transport: &Transport,
    keyword: &str,
    offset: u32,
    limit: u32,
) -> Result<Vec<Playlist>> {
    let raw = search_raw(transport, keyword, 1000, offset, limit).await?;
    let result = raw
        .get("result")
        .ok_or_else(|| anyhow!("search response missing `result`"))?;
    let parsed: SearchPlaylistsResult = serde_json::from_value(result.clone())?;
    Ok(parsed
        .playlists
        .into_iter()
        .map(|p| Playlist {
            source: SourceKind::Netease,
            id: PlaylistId::new(p.id.to_string()),
            name: p.name,
            description: p.description.unwrap_or_default(),
            cover_url: p.cover_img_url.as_deref().and_then(parse_remote),
            track_count: p.track_count,
            songs: Vec::new(),
        })
        .collect())
}

