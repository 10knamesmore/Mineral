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

use std::collections::HashSet;

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

    /// 拉取指定 uid 用户的歌单列表(可选)。
    ///
    /// 用于"看其他人的歌单"等需要显式 uid 的场景;TUI 默认走 [`Self::my_playlists`]。
    async fn user_playlists(&self, _uid: &UserId) -> Result<Vec<Playlist>> {
        Err(Error::NotSupported)
    }

    /// 拉取**该 channel 实例自身上下文中**的"我的歌单"。
    ///
    /// 这是 TUI 跨 channel 平等取数的入口:
    /// - 网易云:实例内部已绑定登录用户 uid,内部转发给 [`Self::user_playlists`]。
    /// - 本地:遍历配置里的扫描根。
    /// - 没有"用户"概念或未登录时:返回 [`Error::NotSupported`]。
    ///
    /// TUI 看到 `NotSupported` 视为该 channel 不贡献歌单,正常继续从其他 channel 拉。
    async fn my_playlists(&self) -> Result<Vec<Playlist>> {
        Err(Error::NotSupported)
    }

    // ---------- 用户数据 / 装饰(可选) ----------
    // 这一组方法都是「同一登录用户视角下,跨歌曲的元信息」,bulk 一次拉满,
    // 上层用来 decorate `SongView`。沿用 default `NotSupported` 模式。
    // 后续按需追加(`user_play_counts` / 关注列表 / 个人评分等)。

    /// 当前用户喜欢(♥)的歌曲 ID 集合(read-only)。
    ///
    /// `liked_song_ids` 是 user-data 类的第一个口子;返回 `HashSet` 而非 `Vec` 是因为
    /// 调用方会做点查("这首歌 like 了吗")。channel 不支持/未登录时返回 [`Error::NotSupported`]。
    async fn liked_song_ids(&self) -> Result<HashSet<SongId>> {
        Err(Error::NotSupported)
    }
}
