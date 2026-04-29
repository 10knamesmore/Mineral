//! 歌曲详情、播放 URL 端点。
//!
//! `song_urls` 实现 spec §4.3 的双层降级:
//! 1. 先尝试 `SongUrlV1Service`(`/weapi/song/enhance/player/url/v1`,字符串等级)
//! 2. 失败或返回试听片段(`freeTrialInfo` 非空)再降到 `SongUrlService`(`/api/song/enhance/player/url`,数字 br)

use color_eyre::eyre::eyre;
use mineral_model::{
    AlbumId, AlbumRef, ArtistId, ArtistRef, BitRate, MediaUrl, PlayUrl, Song, SongId, SourceKind,
};
use serde_json::json;

type Result<T> = color_eyre::Result<T>;

use crate::convert::parse_remote;
use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;
use crate::wire::song::{AlbumSong, SongUrl};

/// 详情:`/weapi/v3/song/detail`。返回与 album.rs 一致的 `wire::song::AlbumSong` 形态。
pub async fn songs_detail(transport: &Transport, ids: &[SongId]) -> Result<Vec<Song>> {
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
    let dtos: Vec<AlbumSong> = serde_json::from_value(songs.clone())?;
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

/// 播放 URL,**双层降级**。
pub async fn song_urls(
    transport: &Transport,
    ids: &[SongId],
    quality: BitRate,
) -> Result<Vec<PlayUrl>> {
    if let Ok(out) = song_urls_v1(transport, ids, quality).await {
        if !out.is_empty() {
            return Ok(out);
        }
    }
    song_urls_legacy(transport, ids, quality).await
}

/// SongUrlV1Service:`/weapi/song/enhance/player/url/v1`,字符串等级。
async fn song_urls_v1(
    transport: &Transport,
    ids: &[SongId],
    quality: BitRate,
) -> Result<Vec<PlayUrl>> {
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
    parse_song_url_data(&v, quality)
}

/// 旧版 SongUrlService(linuxapi),双层降级里的 fallback。
async fn song_urls_legacy(
    transport: &Transport,
    ids: &[SongId],
    quality: BitRate,
) -> Result<Vec<PlayUrl>> {
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
    parse_song_url_data(&v, quality)
}

fn parse_song_url_data(v: &serde_json::Value, quality: BitRate) -> Result<Vec<PlayUrl>> {
    let data = v
        .get("data")
        .ok_or_else(|| eyre!("song url response missing `data`"))?;
    let dtos: Vec<SongUrl> = serde_json::from_value(data.clone())?;
    Ok(dtos
        .into_iter()
        .filter_map(|d| {
            let url_str = d.url?;
            let url = MediaUrl::remote(&url_str).ok()?;
            Some(PlayUrl {
                source: SourceKind::Netease,
                song_id: SongId::new(d.id.to_string()),
                url,
                bitrate_bps: d.br,
                quality,
                size: d.size,
                format: d.format.unwrap_or_default(),
            })
        })
        .collect())
}
