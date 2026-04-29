//! 专辑端点。

use anyhow::{anyhow, Result};
use mineral_model::{AlbumId, AlbumRef, ArtistId, ArtistRef, Song, SongId, SourceKind};

use crate::convert::parse_remote;
use crate::dto::song::AlbumSongDto;
use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;

pub async fn songs_in_album(transport: &Transport, album_id: &AlbumId) -> Result<Vec<Song>> {
    let path = format!("/weapi/v1/album/{}", album_id.as_str());
    let raw = transport
        .request(RequestSpec {
            path: &path,
            crypto: Crypto::Weapi,
            params: serde_json::Map::new(),
            ua: UaKind::Any,
        })
        .await?;
    let songs = raw
        .get("songs")
        .ok_or_else(|| anyhow!("album response missing `songs`"))?;
    let dtos: Vec<AlbumSongDto> = serde_json::from_value(songs.clone())?;
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
