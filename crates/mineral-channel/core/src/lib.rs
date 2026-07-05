//! 抽象的"音乐源" channel trait。
//!
//! 任何具体音乐源(网易云、本地、QQ ……)都通过实现 [`MusicChannel`] 接入。
//! 上层只面向 trait 编程,channel 实现间通过 [`mineral_model`] 中的统一类型互通。

/// channel 能力声明。
pub mod caps;
/// 登录凭证类型。
pub mod credential;
/// channel 公共错误类型与 `Result` 别名。
pub mod error;
/// 列表分页参数。
pub mod page;

pub use caps::ChannelCaps;
pub use credential::Credential;
pub use error::{Error, Result};
pub use page::Page;

use rustc_hash::FxHashSet;

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

    /// 该 channel 的能力声明,见 [`ChannelCaps`]。
    ///
    /// 刻意**不给默认实现**:能力是每个 channel 必须显式表态的事,
    /// 默认值会让新 channel 静默继承错误声明。
    fn caps(&self) -> ChannelCaps;

    // ---------- 搜索 ----------
    /// 搜索单曲。
    async fn search_songs(&self, query: &str, page: Page) -> Result<Vec<Song>>;
    /// 搜索专辑(可选)。
    async fn search_albums(&self, _query: &str, _page: Page) -> Result<Vec<Album>> {
        Err(Error::NotSupported)
    }
    /// 搜索歌单(可选)。
    async fn search_playlists(&self, _query: &str, _page: Page) -> Result<Vec<Playlist>> {
        Err(Error::NotSupported)
    }
    /// 搜索艺人(可选)。
    async fn search_artists(&self, _query: &str, _page: Page) -> Result<Vec<Artist>> {
        Err(Error::NotSupported)
    }

    // ---------- 详情 ----------
    /// 拉取若干歌曲的详情。
    async fn songs_detail(&self, ids: &[SongId]) -> Result<Vec<Song>>;

    /// 拉取一张专辑的完整详情:元信息(简介 / 发行信息 / 曲目数等)+ 曲目列表(`songs`)。
    ///
    /// 返回**完整实体**而非裸曲目:上层要的是"这张专辑的完整视图",至于 channel 内部打几个
    /// 端点拼出来是实现细节。只要曲目的调用方读 `.songs` 即可。
    async fn album_detail(&self, _id: &AlbumId) -> Result<Album> {
        Err(Error::NotSupported)
    }

    /// 拉取一个歌单的完整详情:元信息(名/简介/封面/计数)+ 曲目(`songs`)。
    ///
    /// 同 [`Self::album_detail`] —— 返回完整实体而非裸曲目;只要曲目的调用方读 `.songs`。
    async fn playlist_detail(&self, _id: &PlaylistId) -> Result<Playlist> {
        Err(Error::NotSupported)
    }
    /// 拉取艺人详情(可选)。
    async fn artist_detail(&self, _id: &ArtistId) -> Result<Artist> {
        Err(Error::NotSupported)
    }

    /// 拉取艺人的专辑列表(分页,可选)。
    ///
    /// # Params:
    ///   - `id`: 艺人
    ///   - `page`: 分页参数
    ///
    /// # Return:
    ///   专辑列表;`songs` 留空,曲目按需走 [`Self::album_detail`]。
    async fn artist_albums(&self, _id: &ArtistId, _page: Page) -> Result<Vec<Album>> {
        Err(Error::NotSupported)
    }

    // ---------- 歌单管理(可选写操作) ----------
    // 前置条件(调用方保证,server 在边界校验):涉及的 SongId 必须与歌单
    // PlaylistId 同 namespace——远程歌单装不下别源的歌,channel 实现不做
    // 防御性检查。写操作失败语义见 [`Error`];实现方不得把远端的"已存在"
    // 等业务态伪装成成功。

    /// 创建歌单(可选)。
    ///
    /// # Params:
    ///   - `name`: 歌单名
    ///
    /// # Return:
    ///   新建的歌单。实现方应从创建响应直接映射,避免"建完再拉列表"的额外往返。
    async fn create_playlist(&self, _name: &str) -> Result<Playlist> {
        Err(Error::NotSupported)
    }

    /// 删除自己创建的歌单(可选)。
    async fn delete_playlist(&self, _id: &PlaylistId) -> Result<()> {
        Err(Error::NotSupported)
    }

    /// 向歌单追加歌曲(可选)。
    ///
    /// 歌曲已在歌单中时透传远端语义(如网易云 code 502 的
    /// [`Error::Api`]),由上层翻译,不伪装成功。
    ///
    /// # Params:
    ///   - `id`: 目标歌单
    ///   - `songs`: 待追加歌曲(与歌单同 namespace)
    async fn playlist_add_songs(&self, _id: &PlaylistId, _songs: &[SongId]) -> Result<()> {
        Err(Error::NotSupported)
    }

    /// 从歌单移除歌曲(可选)。
    ///
    /// # Params:
    ///   - `id`: 目标歌单
    ///   - `songs`: 待移除歌曲(与歌单同 namespace)
    async fn playlist_remove_songs(&self, _id: &PlaylistId, _songs: &[SongId]) -> Result<()> {
        Err(Error::NotSupported)
    }

    /// 歌单改名(可选)。
    async fn rename_playlist(&self, _id: &PlaylistId, _name: &str) -> Result<()> {
        Err(Error::NotSupported)
    }

    /// 修改歌单描述(可选)。
    async fn set_playlist_description(&self, _id: &PlaylistId, _desc: &str) -> Result<()> {
        Err(Error::NotSupported)
    }

    // ---------- 播放 ----------
    /// 解析若干歌曲在指定音质下的播放 URL。
    async fn song_urls(&self, ids: &[SongId], quality: BitRate) -> Result<Vec<PlayUrl>>;
    /// 拉取一首歌的歌词(可选)。
    async fn lyrics(&self, _id: &SongId) -> Result<Lyrics> {
        Err(Error::NotSupported)
    }

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
    /// `liked_song_ids` 是 user-data 类的第一个口子;返回 `FxHashSet` 而非 `Vec` 是因为
    /// 调用方会做点查("这首歌 like 了吗")。channel 不支持/未登录时返回 [`Error::NotSupported`]。
    async fn liked_song_ids(&self) -> Result<FxHashSet<SongId>> {
        Err(Error::NotSupported)
    }

    /// 设置 / 取消一首歌的喜欢(♥)。
    ///
    /// 命令透传由 channel 自行决定:本地 channel 只写持久化;网易云既写持久化、
    /// 又打远端红心接口。channel 不支持 / 未登录时返回 [`Error::NotSupported`]。
    ///
    /// # Params:
    ///   - `id`: 目标歌曲
    ///   - `loved`: `true` 喜欢、`false` 取消
    async fn set_loved(&self, _id: &SongId, _loved: bool) -> Result<()> {
        Err(Error::NotSupported)
    }

    /// 拉取该 channel 远端记录的「当前用户对这首歌的真实累计播放次数」。
    ///
    /// 语义上**与本地 persist 统计无关**:这是音乐源服务端的事实(如网易云
    /// 网页 + App + 本客户端的全渠道累计),而非本地这台机器上播了几次。
    /// 默认 [`Error::NotSupported`](而非 `Ok(0)`),让上层能区分「该源无此能力 /
    /// 未登录」(不显示)与「确实播了 0 次」(显示 0)。
    ///
    /// # Params:
    ///   - `id`: 目标歌曲
    ///
    /// # Return:
    ///   远端累计播放次数;不支持 / 未登录返回 [`Error::NotSupported`]。
    async fn remote_play_count(&self, _id: &SongId) -> Result<u32> {
        Err(Error::NotSupported)
    }

    /// 播放打点(fire-and-forget 语义):一首歌完整播完或被跳过时上报。
    ///
    /// channel 据此累计本地统计(播放次数 / 跳过 / 收听时长 / 历史),也可顺手做
    /// 远端听歌打卡。默认 no-op(返回 `Ok`),不支持的 channel 静默忽略。
    ///
    /// # Params:
    ///   - `id`: 歌曲
    ///   - `completed`: 是否完整播完(`false` = 被跳过)
    ///   - `listen_ms`: 本次收听毫秒
    async fn on_played(&self, _id: &SongId, _completed: bool, _listen_ms: u64) -> Result<()> {
        Ok(())
    }
}
