//! 占位伪 channel — 实现 [`mineral_channel_core::MusicChannel`] trait,
//! 提供编译期常量级的 fake 数据,供 UI / 测试在未接入真实 channel 时使用。
//!
//! 整个 crate 的内容只在自身 `mock` feature 启用时存在;feature off 时
//! lib 是空 stub。这保证 workspace 检查不会通过 feature unification 把
//! `SourceKind::Mock` 渗透到不需要的 crate 里。

#![cfg(feature = "mock")]

use async_trait::async_trait;
use mineral_channel_core::{Credential, Error, MusicChannel, Page, Result};
use mineral_model::{
    Album, AlbumId, AlbumRef, Artist, ArtistId, ArtistRef, BitRate, Lyrics, PlayUrl, Playlist,
    PlaylistId, Song, SongId, SourceKind, UserId,
};

/// 一首 mock 歌 + UI 装饰(loved / 累计播放次数)。
#[derive(Clone, Debug)]
pub struct DemoSong {
    /// 底层 model。
    pub data: Song,
    /// 是否已收藏。
    pub loved: bool,
    /// 累计播放次数(完全是 mock 值)。
    pub plays: u32,
}

/// 一条完整 demo 歌单:`Playlist` model + 已 decorated 的曲目列表。
#[derive(Clone, Debug)]
pub struct DemoPlaylist {
    /// 底层 model(`songs` 字段已填同 `tracks` 同序的曲目)。
    pub data: Playlist,
    /// 装饰过的曲目(`tracks[i].data == data.songs[i]`)。
    pub tracks: Vec<DemoSong>,
}

/// mock channel 实现。预先在 `new()` 时构造好全部 demo 数据。
#[derive(Clone, Debug)]
pub struct MockChannel {
    playlists: Vec<DemoPlaylist>,
}

impl MockChannel {
    /// 构造 mock channel(数据是常量,瞬时完成)。
    pub fn new() -> Self {
        Self {
            playlists: build_demo_playlists(),
        }
    }

    /// 同步取所有 demo 歌单(包括 loved / plays 等 UI 装饰)。
    pub fn demo_playlists(&self) -> &[DemoPlaylist] {
        &self.playlists
    }

    fn find_playlist(&self, id: &PlaylistId) -> Option<&DemoPlaylist> {
        self.playlists.iter().find(|p| &p.data.id == id)
    }
}

impl Default for MockChannel {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MusicChannel for MockChannel {
    fn source(&self) -> SourceKind {
        SourceKind::Mock
    }

    async fn search_songs(&self, query: &str, _page: Page) -> Result<Vec<Song>> {
        let q = query.to_lowercase();
        Ok(self
            .playlists
            .iter()
            .flat_map(|p| p.tracks.iter().map(|t| t.data.clone()))
            .filter(|s| s.name.to_lowercase().contains(&q))
            .collect())
    }

    async fn search_albums(&self, _query: &str, _page: Page) -> Result<Vec<Album>> {
        Err(Error::NotSupported)
    }

    async fn search_playlists(&self, query: &str, _page: Page) -> Result<Vec<Playlist>> {
        let q = query.to_lowercase();
        Ok(self
            .playlists
            .iter()
            .filter(|p| p.data.name.to_lowercase().contains(&q))
            .map(|p| p.data.clone())
            .collect())
    }

    async fn songs_detail(&self, ids: &[SongId]) -> Result<Vec<Song>> {
        Ok(self
            .playlists
            .iter()
            .flat_map(|p| p.tracks.iter().map(|t| &t.data))
            .filter(|s| ids.iter().any(|id| id == &s.id))
            .cloned()
            .collect())
    }

    async fn songs_in_album(&self, _id: &AlbumId) -> Result<Vec<Song>> {
        Err(Error::NotSupported)
    }

    async fn songs_in_playlist(&self, id: &PlaylistId) -> Result<Vec<Song>> {
        Ok(self
            .find_playlist(id)
            .map(|p| p.tracks.iter().map(|t| t.data.clone()).collect())
            .unwrap_or_default())
    }

    async fn song_urls(&self, _ids: &[SongId], _quality: BitRate) -> Result<Vec<PlayUrl>> {
        Err(Error::NotSupported)
    }

    async fn lyrics(&self, _id: &SongId) -> Result<Lyrics> {
        Ok(Lyrics::default())
    }

    async fn login(&self, _credential: Credential) -> Result<()> {
        Ok(())
    }

    async fn user_playlists(&self, _uid: &UserId) -> Result<Vec<Playlist>> {
        Ok(self.playlists.iter().map(|p| p.data.clone()).collect())
    }

    async fn artist_detail(&self, _id: &ArtistId) -> Result<Artist> {
        Err(Error::NotSupported)
    }
}

/// 标题池 — 用于 round-robin 给曲目命名。
const TITLES: &[&str] = &[
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

fn build_demo_playlists() -> Vec<DemoPlaylist> {
    [
        ("All Time Favorites", 24),
        ("Recently Added", 12),
        ("Loved", 18),
        ("Late Night Drive", 16),
        ("Synthwave", 22),
        ("Ambient Focus", 10),
        ("Workout Mix", 14),
    ]
    .iter()
    .map(|(name, n)| build_one(name, *n))
    .collect()
}

fn build_one(name: &str, count: usize) -> DemoPlaylist {
    let songs = build_songs(name, "Various Artists", count);
    let tracks = decorate(&songs);
    let playlist = Playlist {
        source: SourceKind::Mock,
        id: PlaylistId::new(name.to_owned()),
        name: name.to_owned(),
        description: String::new(),
        cover_url: None,
        track_count: u64::try_from(count).unwrap_or(0),
        songs,
    };
    DemoPlaylist {
        data: playlist,
        tracks,
    }
}

fn build_songs(album_name: &str, artist_name: &str, count: usize) -> Vec<Song> {
    let artist_ref = ArtistRef {
        id: ArtistId::new(format!("artist:{artist_name}")),
        name: artist_name.to_owned(),
    };
    let album_ref = AlbumRef {
        id: AlbumId::new(format!("album:{album_name}")),
        name: album_name.to_owned(),
    };
    (0..count)
        .map(|i| Song {
            source: SourceKind::Mock,
            id: SongId::new(format!("{album_name}/{i}")),
            name: TITLES
                .get(i % TITLES.len())
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

fn decorate(songs: &[Song]) -> Vec<DemoSong> {
    songs
        .iter()
        .enumerate()
        .map(|(i, s)| DemoSong {
            data: s.clone(),
            loved: i.is_multiple_of(3),
            plays: 7_u32.saturating_mul(u32::try_from(i + 1).unwrap_or(0)),
        })
        .collect()
}
