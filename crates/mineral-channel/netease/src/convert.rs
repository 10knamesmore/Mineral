//! 网易原生 DTO → `mineral_model` 类型的转换 helper。

use mineral_model::{AlbumId, AlbumRef, ArtistId, ArtistRef, MediaUrl, Song, SongId, SourceKind};

use crate::wire::song::AlbumSong;

/// 把网易 JSON 里的字符串字段(永远是 http(s) URL)转成 `MediaUrl::Remote`。
///
/// 解析失败或空字符串返回 `None`,让 `Option<MediaUrl>` 字段保持空。
pub fn parse_remote(s: &str) -> Option<MediaUrl> {
    if s.is_empty() {
        return None;
    }
    MediaUrl::remote(s).ok()
}

/// `Option<&str>` 版的便利包装。
pub fn parse_remote_opt(s: Option<&str>) -> Option<MediaUrl> {
    s.and_then(parse_remote)
}

/// 专辑/歌单/歌手详情里的 [`AlbumSong`](ar/al/dt 字段风格)→ 统一 [`Song`]。
pub(crate) fn album_song_to_model(s: AlbumSong) -> Song {
    Song {
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
    }
}
