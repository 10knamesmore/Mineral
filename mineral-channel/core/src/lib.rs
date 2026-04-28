//! 抽象的"音乐源" channel trait。
//!
//! 任何具体音乐源(网易云、本地、QQ ……)都通过实现 [`MusicChannel`] 接入。
//! 上层只面向 trait 编程,channel 实现间通过 [`mineral_model`] 中的统一类型互通。

/// 登录凭证类型。
pub mod credential;
/// channel 公共错误类型与 `Result` 别名。
pub mod error;
/// 列表分页参数。
pub mod page;

pub use credential::Credential;
pub use error::{Error, Result};
pub use page::Page;

use async_trait::async_trait;
use mineral_model::{
    Album, AlbumId, Artist, ArtistId, BitRate, Lyrics, PlayUrl, Playlist, PlaylistId, Song, SongId,
    SourceKind, UserId,
};

/// 一个音乐源 channel 的统一接口。
///
/// 所有方法都是异步、可独立失败的;不支持的能力直接返回 [`Error::NotSupported`]。
#[async_trait]
pub trait MusicChannel: Send + Sync {
    /// 该 channel 的来源标识。
    fn source(&self) -> SourceKind;

    // ---------- 搜索 ----------
    /// 搜索单曲。
    async fn search_songs(&self, query: &str, page: Page) -> Result<Vec<Song>>;
    /// 搜索专辑。
    async fn search_albums(&self, query: &str, page: Page) -> Result<Vec<Album>>;
    /// 搜索歌单。
    async fn search_playlists(&self, query: &str, page: Page) -> Result<Vec<Playlist>>;
    /// 搜索艺人(可选)。
    async fn search_artists(&self, _query: &str, _page: Page) -> Result<Vec<Artist>> {
        Err(Error::NotSupported)
    }

    // ---------- 详情 ----------
    /// 拉取若干歌曲的详情。
    async fn songs_detail(&self, ids: &[SongId]) -> Result<Vec<Song>>;
    /// 拉取专辑下的全部曲目。
    async fn songs_in_album(&self, id: &AlbumId) -> Result<Vec<Song>>;
    /// 拉取歌单下的全部曲目。
    async fn songs_in_playlist(&self, id: &PlaylistId) -> Result<Vec<Song>>;
    /// 拉取艺人详情(可选)。
    async fn artist_detail(&self, _id: &ArtistId) -> Result<Artist> {
        Err(Error::NotSupported)
    }

    // ---------- 播放 ----------
    /// 解析若干歌曲在指定音质下的播放 URL。
    async fn song_urls(&self, ids: &[SongId], quality: BitRate) -> Result<Vec<PlayUrl>>;
    /// 拉取一首歌的歌词。
    async fn lyrics(&self, id: &SongId) -> Result<Lyrics>;

    // ---------- 用户 / 登录(可选) ----------
    /// 用给定凭证登录(可选)。
    async fn login(&self, _credential: Credential) -> Result<()> {
        Err(Error::NotSupported)
    }
    /// 拉取用户的歌单列表(需要登录,可选)。
    async fn user_playlists(&self, _uid: &UserId) -> Result<Vec<Playlist>> {
        Err(Error::NotSupported)
    }
}
