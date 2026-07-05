//! 占位伪 channel — 实现 [`mineral_channel_core::MusicChannel`] trait,
//! 提供编译期常量级的 fake 数据,供 UI / 测试在未接入真实 channel 时使用。
//!
//! 整个 crate 的内容只在自身 `mock` feature 启用时存在;feature off 时
//! lib 是空 stub。这保证 workspace 检查不会通过 feature unification 把
//! `SourceKind::MOCK` 渗透到不需要的 crate 里。

#![cfg(feature = "mock")]

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use mineral_channel_core::{
    ArtistSectionKind, ArtistSections, ChannelCaps, Credential, Error, MusicChannel, Page, Result,
    SearchHits,
};
use mineral_model::{
    Album, AlbumId, AlbumRef, Artist, ArtistId, ArtistRef, BitRate, Lyrics, PlayUrl, Playlist,
    PlaylistId, SearchKind, Song, SongId, SourceKind, UserId,
};
use parking_lot::RwLock;

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

/// mock channel 实现。预制 demo 数据在 `new()` 时构造,歌单写操作就地
/// 修改内存(进程退出即丢,mock 不做持久化)。
#[derive(Debug)]
pub struct MockChannel {
    /// 全部歌单(预制 demo + 运行期用户新建)。
    playlists: RwLock<Vec<DemoPlaylist>>,

    /// 建单自增序号:同名歌单也要有不同 id。
    next_playlist_seq: AtomicU64,
}

impl MockChannel {
    /// 构造 mock channel(数据是常量,瞬时完成)。
    pub fn new() -> Self {
        Self {
            playlists: RwLock::new(build_demo_playlists()),
            next_playlist_seq: AtomicU64::new(0),
        }
    }
}

/// 歌单不存在时的统一错误(mock 自拟 404,形态对齐 netease 的 `Error::Api`)。
fn missing_playlist(id: &PlaylistId) -> Error {
    Error::Api {
        code: 404,
        message: format!("歌单不存在: {}", id.qualified()),
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
        SourceKind::MOCK
    }

    fn caps(&self) -> ChannelCaps {
        ChannelCaps::builder()
            .searchable(vec![SearchKind::Song, SearchKind::Playlist])
            .playlist_edit(true)
            .artist_sections(ArtistSections::new(vec![
                ArtistSectionKind::TopSongs,
                ArtistSectionKind::Albums,
            ]))
            .song_web_url(Some("https://mock.example/song/{id}".to_owned()))
            .playlist_web_url(Some("https://mock.example/playlist/{id}".to_owned()))
            .build()
    }

    async fn search_songs(&self, query: &str, _page: Page) -> Result<SearchHits<Song>> {
        let q = query.to_lowercase();
        let items = self
            .playlists
            .read()
            .iter()
            .flat_map(|p| p.tracks.iter().map(|t| t.data.clone()))
            .filter(|s| s.name.to_lowercase().contains(&q))
            .collect::<Vec<Song>>();
        // 一次性回全量,显式封死翻页(否则命中 ≥ limit 时上层会无限续拉同一批)。
        Ok(SearchHits::new(items, /*has_more*/ false))
    }

    async fn search_albums(&self, _query: &str, _page: Page) -> Result<SearchHits<Album>> {
        Err(Error::NotSupported)
    }

    async fn search_playlists(&self, query: &str, _page: Page) -> Result<SearchHits<Playlist>> {
        let q = query.to_lowercase();
        let items = self
            .playlists
            .read()
            .iter()
            .filter(|p| p.data.name.to_lowercase().contains(&q))
            .map(|p| p.data.clone())
            .collect::<Vec<Playlist>>();
        Ok(SearchHits::new(items, /*has_more*/ false))
    }

    async fn songs_detail(&self, ids: &[SongId]) -> Result<Vec<Song>> {
        Ok(self
            .playlists
            .read()
            .iter()
            .flat_map(|p| p.tracks.iter().map(|t| &t.data))
            .filter(|s| ids.iter().any(|id| id == &s.id))
            .cloned()
            .collect())
    }

    async fn album_detail(&self, _id: &AlbumId) -> Result<Album> {
        Err(Error::NotSupported)
    }

    async fn playlist_detail(&self, id: &PlaylistId) -> Result<Playlist> {
        Ok(self
            .playlists
            .read()
            .iter()
            .find(|p| &p.data.id == id)
            .map(|p| {
                let mut pl = p.data.clone();
                pl.songs = p.tracks.iter().map(|t| t.data.clone()).collect();
                pl
            })
            .unwrap_or_else(|| {
                Playlist::builder()
                    .id(id.clone())
                    .name(String::new())
                    .build()
            }))
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
        Ok(self
            .playlists
            .read()
            .iter()
            .map(|p| p.data.clone())
            .collect())
    }

    async fn my_playlists(&self) -> Result<Vec<Playlist>> {
        Ok(self
            .playlists
            .read()
            .iter()
            .map(|p| p.data.clone())
            .collect())
    }

    async fn artist_detail(&self, _id: &ArtistId) -> Result<Artist> {
        Err(Error::NotSupported)
    }

    async fn create_playlist(&self, name: &str) -> Result<Playlist> {
        let seq = self.next_playlist_seq.fetch_add(1, Ordering::Relaxed);
        let playlist = Playlist::builder()
            .id(PlaylistId::new(SourceKind::MOCK, format!("user-{seq}")))
            .name(name.to_owned())
            .build();
        self.playlists.write().push(DemoPlaylist {
            data: playlist.clone(),
            tracks: Vec::new(),
        });
        Ok(playlist)
    }

    async fn delete_playlist(&self, id: &PlaylistId) -> Result<()> {
        let mut lists = self.playlists.write();
        let before = lists.len();
        lists.retain(|p| &p.data.id != id);
        if lists.len() == before {
            return Err(missing_playlist(id));
        }
        Ok(())
    }

    async fn playlist_add_songs(&self, id: &PlaylistId, songs: &[SongId]) -> Result<()> {
        let mut lists = self.playlists.write();
        // 素材从全库 resolve(加进新歌单的歌必然在某个 demo 歌单里有 meta)
        let resolved = songs
            .iter()
            .map(|sid| {
                lists
                    .iter()
                    .flat_map(|p| p.tracks.iter())
                    .find(|t| &t.data.id == sid)
                    .map(|t| t.data.clone())
                    .ok_or_else(|| Error::Api {
                        code: 404,
                        message: format!("歌曲不存在: {}", sid.qualified()),
                    })
            })
            .collect::<Result<Vec<Song>>>()?;
        let target = lists
            .iter_mut()
            .find(|p| &p.data.id == id)
            .ok_or_else(|| missing_playlist(id))?;
        // 模拟网易云"歌曲已存在"语义:任一重复则整批拒绝,内容不变
        if resolved
            .iter()
            .any(|s| target.data.songs.iter().any(|e| e.id == s.id))
        {
            return Err(Error::Api {
                code: 502,
                message: String::from("歌曲已存在"),
            });
        }
        for song in resolved {
            target.data.songs.push(song.clone());
            target.tracks.push(DemoSong {
                data: song,
                loved: false,
                plays: 0,
            });
        }
        target.data.track_count = u64::try_from(target.data.songs.len()).unwrap_or(0);
        Ok(())
    }

    async fn playlist_remove_songs(&self, id: &PlaylistId, songs: &[SongId]) -> Result<()> {
        let mut lists = self.playlists.write();
        let target = lists
            .iter_mut()
            .find(|p| &p.data.id == id)
            .ok_or_else(|| missing_playlist(id))?;
        // 不在歌单里的歌宽容忽略(对齐"删除是幂等清理"的远端体感)
        target.data.songs.retain(|s| !songs.contains(&s.id));
        target.tracks.retain(|t| !songs.contains(&t.data.id));
        target.data.track_count = u64::try_from(target.data.songs.len()).unwrap_or(0);
        Ok(())
    }

    async fn rename_playlist(&self, id: &PlaylistId, name: &str) -> Result<()> {
        let mut lists = self.playlists.write();
        let target = lists
            .iter_mut()
            .find(|p| &p.data.id == id)
            .ok_or_else(|| missing_playlist(id))?;
        target.data.name = name.to_owned();
        Ok(())
    }

    async fn set_playlist_description(&self, id: &PlaylistId, desc: &str) -> Result<()> {
        let mut lists = self.playlists.write();
        let target = lists
            .iter_mut()
            .find(|p| &p.data.id == id)
            .ok_or_else(|| missing_playlist(id))?;
        target.data.description = desc.to_owned();
        Ok(())
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

/// 一次性构造全部 demo 歌单(名字与歌数预置在源码里)。
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

/// 构造一个 demo 歌单:`count` 首歌 + 装饰(loved/plays)。
fn build_one(name: &str, count: usize) -> DemoPlaylist {
    let songs = build_songs(name, "Various Artists", count);
    let tracks = decorate(&songs);
    let playlist = Playlist::builder()
        .id(PlaylistId::new(SourceKind::MOCK, name.to_owned()))
        .name(name.to_owned())
        .track_count(u64::try_from(count).unwrap_or(0))
        .songs(songs)
        .build();
    DemoPlaylist {
        data: playlist,
        tracks,
    }
}

/// 按指定 artist/album 名生成 `count` 首假歌(标题循环复用 TITLES 列表)。
fn build_songs(album_name: &str, artist_name: &str, count: usize) -> Vec<Song> {
    let artist_ref = ArtistRef {
        id: ArtistId::new(SourceKind::MOCK, format!("artist:{artist_name}")),
        name: artist_name.to_owned(),
    };
    let album_ref = AlbumRef {
        id: AlbumId::new(SourceKind::MOCK, format!("album:{album_name}")),
        name: album_name.to_owned(),
    };
    (0..count)
        .map(|i| {
            Song::builder()
                .id(SongId::new(SourceKind::MOCK, format!("{album_name}/{i}")))
                .name(
                    TITLES
                        .get(i % TITLES.len())
                        .copied()
                        .unwrap_or("Untitled")
                        .to_owned(),
                )
                .artists(vec![artist_ref.clone()])
                .album(Some(album_ref.clone()))
                .duration_ms(180_000 + (u64::try_from(i).unwrap_or(0) * 17_000))
                .build()
        })
        .collect()
}

/// 给一组 song 附上装饰字段:每 3 首一个 loved,plays 按 idx 线性递增。
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
