//! 阶段 3 的占位数据 — 全部用 [`mineral_model`] 真类型构造,后续接入
//! 真实 channel(Netease / 本地扫描)时只换数据源。

use mineral_model::{
    AlbumId, AlbumRef, ArtistId, ArtistRef, MediaUrl, Playlist, PlaylistId, Song, SongId,
    SourceKind,
};

use crate::state::PlaylistView;

/// 歌单类型 — 对应设计稿的 ★/◆/#/♪ 字形。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaylistKind {
    /// 系统(★)。
    System,
    /// 智能(◆)。
    Smart,
    /// 流派(#)。
    Genre,
    /// 用户(♪)。
    User,
}

impl PlaylistKind {
    /// 类型字形。
    pub fn glyph(self) -> &'static str {
        match self {
            Self::System => "★",
            Self::Smart => "◆",
            Self::Genre => "#",
            Self::User => "♪",
        }
    }
}

/// 一首歌 + UI 装饰(love / plays count) — 因 [`mineral_model::Song`]
/// 不含这些字段。
#[derive(Clone, Debug)]
pub struct SongView {
    /// 底层 model。
    pub data: Song,
    /// 是否已收藏。
    pub loved: bool,
    /// 累计播放次数(mock)。
    pub plays: u32,
}

/// 构造若干首假歌。
fn fake_songs(album_name: &str, artist_name: &str, count: usize) -> Vec<Song> {
    let artist_ref = ArtistRef {
        id: ArtistId::new(format!("artist:{artist_name}")),
        name: artist_name.to_owned(),
    };
    let album_ref = AlbumRef {
        id: AlbumId::new(format!("album:{album_name}")),
        name: album_name.to_owned(),
    };
    let titles = [
        "Aurora",
        "Driftwood",
        "Late Night",
        "Static Bloom",
        "Glass Garden",
        "Velvet Sky",
        "Saudade",
        "Long Way Home",
        "Cinder",
        "Halcyon Days",
        "Lowtide",
        "Quartz",
    ];
    (0..count)
        .map(|i| Song {
            source: SourceKind::Local,
            id: SongId::new(format!("{album_name}/{i}")),
            name: titles
                .get(i % titles.len())
                .copied()
                .unwrap_or("Untitled")
                .to_owned(),
            artists: vec![artist_ref.clone()],
            album: Some(album_ref.clone()),
            duration_ms: 180_000 + (u64::try_from(i).unwrap_or(0) * 17_000),
            cover_url: None,
            source_url: None,
        })
        .collect()
}

/// 构造若干 mock 歌单(覆盖 4 种 kind)。
pub fn fake_playlists() -> Vec<PlaylistView> {
    [
        (
            "All Time Favorites",
            PlaylistKind::System,
            "Various Artists",
            24,
        ),
        (
            "Recently Added",
            PlaylistKind::System,
            "Various Artists",
            12,
        ),
        ("Loved", PlaylistKind::Smart, "Various Artists", 18),
        (
            "Late Night Drive",
            PlaylistKind::Genre,
            "Various Artists",
            16,
        ),
        ("Synthwave", PlaylistKind::Genre, "Various Artists", 22),
        ("Ambient Focus", PlaylistKind::User, "Various Artists", 10),
        ("Workout Mix", PlaylistKind::User, "Various Artists", 14),
    ]
    .iter()
    .map(|(name, kind, artist, n)| PlaylistView {
        data: Playlist {
            source: SourceKind::Local,
            id: PlaylistId::new((*name).to_owned()),
            name: (*name).to_owned(),
            description: String::new(),
            cover_url: cover_for(name),
            track_count: u64::try_from(*n).unwrap_or(0),
            songs: fake_songs(name, artist, *n),
        },
        kind: *kind,
    })
    .collect()
}

fn cover_for(_name: &str) -> Option<MediaUrl> {
    None
}

/// 给一组 [`Song`] 附加 mock UI 装饰。
pub fn decorate_songs(songs: &[Song]) -> Vec<SongView> {
    songs
        .iter()
        .enumerate()
        .map(|(i, s)| SongView {
            data: s.clone(),
            loved: i.is_multiple_of(3),
            plays: 7_u32.saturating_mul(u32::try_from(i + 1).unwrap_or(0)),
        })
        .collect()
}
