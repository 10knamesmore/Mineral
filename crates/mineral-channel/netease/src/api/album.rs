//! 专辑端点。

use color_eyre::eyre::eyre;
use mineral_model::{AlbumId, AlbumRef, ArtistId, ArtistRef, Song, SongId, SourceKind};

/// 本模块内部统一的 result 别名,屏蔽 color-eyre 全名。
type Result<T> = color_eyre::Result<T>;

use crate::convert::parse_remote;
use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;
use crate::wire::song::AlbumSong;

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
        .ok_or_else(|| eyre!("album response missing `songs`"))?;
    let dtos: Vec<AlbumSong> = crate::wire::de::from_value(songs.clone())?;
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
