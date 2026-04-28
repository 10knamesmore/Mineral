//! 抽象的"音乐源" channel trait。
//!
//! 任何具体音乐源(网易云、本地、QQ ……)都通过实现 [`MusicChannel`] 接入。
//! 上层只面向 trait 编程,channel 实现间通过 [`mineral_model`] 中的统一类型互通。

pub mod credential;
pub mod error;
pub mod page;

pub use credential::Credential;
pub use error::{Error, Result};
pub use page::Page;

use async_trait::async_trait;
use mineral_model::{
    Album, AlbumId, Artist, ArtistId, BitRate, Lyrics, PlayUrl, Playlist, PlaylistId, Song, SongId,
    SourceKind, UserId,
};

#[async_trait]
pub trait MusicChannel: Send + Sync {
    /// 该 channel 的来源标识。
    fn source(&self) -> SourceKind;

    // ---------- 搜索 ----------
    async fn search_songs(&self, query: &str, page: Page) -> Result<Vec<Song>>;
    async fn search_albums(&self, query: &str, page: Page) -> Result<Vec<Album>>;
    async fn search_playlists(&self, query: &str, page: Page) -> Result<Vec<Playlist>>;
    async fn search_artists(&self, _query: &str, _page: Page) -> Result<Vec<Artist>> {
        Err(Error::NotSupported)
    }

    // ---------- 详情 ----------
    async fn songs_detail(&self, ids: &[SongId]) -> Result<Vec<Song>>;
    async fn songs_in_album(&self, id: &AlbumId) -> Result<Vec<Song>>;
    async fn songs_in_playlist(&self, id: &PlaylistId) -> Result<Vec<Song>>;
    async fn artist_detail(&self, _id: &ArtistId) -> Result<Artist> {
        Err(Error::NotSupported)
    }

    // ---------- 播放 ----------
    async fn song_urls(&self, ids: &[SongId], quality: BitRate) -> Result<Vec<PlayUrl>>;
    async fn lyrics(&self, id: &SongId) -> Result<Lyrics>;

    // ---------- 用户 / 登录(可选) ----------
    async fn login(&self, _credential: Credential) -> Result<()> {
        Err(Error::NotSupported)
    }
    async fn user_playlists(&self, _uid: &UserId) -> Result<Vec<Playlist>> {
        Err(Error::NotSupported)
    }
}
