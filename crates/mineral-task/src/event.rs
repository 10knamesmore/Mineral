//! 任务推到 client 的事件载荷。

use mineral_channel_core::Page;
use mineral_model::{
    Album, AlbumId, Artist, ArtistId, Lyrics, Playlist, PlaylistId, SearchKind, Song, SongId,
    SourceKind,
};
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};

use crate::ChannelFetchKind;
use crate::kind::ChannelFetchKindTag;
use crate::outcome::TaskOutcome;
use crate::write::{PlaylistWriteOp, WriteError};
use mineral_model::{CollectionCategory, RefreshScope};

/// 任务结果与服务端快照；列表请求可在刷新完成前发布缓存。
///
/// 搜索或艺人专辑页失败、取消时，分别发 [`TaskEvent::SearchPageFailed`] 或
/// [`TaskEvent::ArtistAlbumsPageFailed`]，client 据此清除待收取页标记。
/// 歌单写操作的失败由 [`TaskEvent::PlaylistWriteDone`] 携带结构化错误。
/// 所有 channel 取数终态另发 [`TaskEvent::FetchDone`] 供 server 记录埋点，详细错误进 mineral-log。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TaskEvent {
    /// 某来源的一份完整歌单列表；同一请求可先发布缓存，再发布远端结果。
    PlaylistsFetched {
        /// 来源(source)。
        source: SourceKind,

        /// 拉到的歌单。
        playlists: Vec<Playlist>,
    },

    /// 用户歌单库合并快照:server 聚合各源列表(经出口变换)后的整表,
    /// **全量替换**语义,顺序即展示序。非任务事件(server 聚合态直接推,
    /// 不经 channel-fetch lane);[`TaskEvent::PlaylistsFetched`] 是它的
    /// server 内部原料,不直接转发 client。
    LibrarySnapshot {
        /// 合并后的歌单列表(展示序)。
        playlists: Vec<Playlist>,
    },

    /// 某源 canonical 收藏(♥)集已到:server 从本地 persist 读、经远端红心导入合并后推送,
    /// client 据此装饰 Song / PlaylistEntry view。非任务事件(server 直接推,不经
    /// channel-fetch lane)。
    LikedSongIdsFetched {
        /// 来源(source)。
        source: SourceKind,

        /// 已收藏的歌曲 ID 集合(canonical,以本地 persist 为准)。
        ids: FxHashSet<SongId>,
    },

    /// 歌单首批、续页或完整加载已到；消费者必须检查结果的完整性。
    PlaylistDetailFetched {
        /// 歌单 id。
        id: PlaylistId,

        /// 本次请求的加载意图。
        load: mineral_channel_core::PlaylistLoad,

        /// 有效缓存或远端结果为 true；false 仅供展示，刷新仍在进行。
        fresh: bool,

        /// 元信息、曲目及完整性。
        detail: Box<mineral_channel_core::PlaylistDetail>,
    },

    /// 歌单请求失败或取消；保留已展示的曲目，释放对应在途状态。
    PlaylistDetailFailed {
        /// 目标歌单。
        id: PlaylistId,

        /// 失败请求的加载意图。
        load: mineral_channel_core::PlaylistLoad,
    },

    /// `Lyrics` 任务成功:歌词数据就绪。
    LyricsReady {
        /// 关联的歌曲 id。
        song_id: SongId,

        /// 各格式歌词(LRC / yrc / 翻译 / 罗马音)。
        lyrics: Lyrics,
    },

    /// `RemotePlayCount` 任务成功:某首歌的远端真实累计播放次数已到。
    RemotePlayCountFetched {
        /// 关联的歌曲 id。
        song_id: SongId,

        /// 远端累计播放次数。
        count: u32,
    },

    /// Mineral 本地完整播放次数查询完成。
    ///
    /// 这是 client 后台查询的 completion event，不经 channel-fetch lane。
    LocalPlayCountFetched {
        /// 关联的歌曲 id。
        song_id: SongId,

        /// 自然播完次数；`None` 表示当前不采集或查询不可用。
        count: Option<u32>,
    },

    /// `Search` 任务成功:一页搜索结果已到。
    ///
    /// 回带完整请求四元组,client 据此配对到对应搜索会话;query 已变的
    /// 过期响应由 client 直接丢弃。
    SearchResults {
        /// 来源(source)。
        source: SourceKind,

        /// 搜索实体类型。
        kind: SearchKind,

        /// 关键词。
        query: String,

        /// 分页参数。
        page: Page,

        /// 结果载荷(变体与 `kind` 一致)。
        payload: SearchPayload,

        /// channel 的显式翻页信号(`PageResult::has_more` 透传):`None` = 源不知道,
        /// client 回退「返回条数 < limit 即榨干」推断。
        #[serde(default)]
        has_more: Option<bool>,
    },

    /// `Search` 任务失败或取消，client 据此释放对应请求的待收取页标记。
    ///
    /// 回带与 [`TaskEvent::SearchResults`] 相同的请求四元组，不携带内部错误文本。
    SearchPageFailed {
        /// 来源(source)。
        source: SourceKind,

        /// 搜索实体类型。
        kind: SearchKind,

        /// 关键词。
        query: String,

        /// 分页参数。
        page: Page,
    },

    /// `ArtistDetail` 任务成功:artist 简介 + 热门曲目已到。
    ArtistDetailFetched {
        /// artist id。
        id: ArtistId,

        /// artist 详情。
        artist: Box<Artist>,
    },

    /// `ArtistAlbums` 任务成功:artist 的一页专辑列表已到。
    ArtistAlbumsFetched {
        /// artist id。
        id: ArtistId,

        /// 分页参数(client 配对翻页用)。
        page: Page,

        /// 专辑列表(曲目留空)。
        albums: Vec<Album>,

        /// channel 的翻页信号；`None` 时 client 按返回条数与请求 `limit` 判断末页。
        has_more: Option<bool>,
    },

    /// `ArtistAlbums` 任务失败或取消，client 据此释放对应艺人、页请求的待收取标记。
    ///
    /// 回带与 [`TaskEvent::ArtistAlbumsFetched`] 相同的艺人和分页参数，不携带内部错误文本。
    ArtistAlbumsPageFailed {
        /// 艺人 id，包含来源身份。
        id: ArtistId,

        /// 失败或取消的分页请求。
        page: Page,
    },

    /// 专辑完整详情快照(元信息 + 曲目)已到；fresh 表示本次读取已确认有效。
    AlbumDetailFetched {
        /// 专辑 id。
        id: AlbumId,

        /// 有效缓存或远端结果为 true；false 仅供展示，刷新仍在进行。
        fresh: bool,

        /// 完整专辑(含曲目)。
        album: Box<Album>,
    },

    /// `PlaylistWrite` 任务完结(**成功失败都发**,见模块文档)。
    PlaylistWriteDone {
        /// 原操作回带(client 据此定位 pending 项与 toast 文案)。
        op: PlaylistWriteOp,

        /// 失败时的结构化错误;`None` = 成功。
        error: Option<WriteError>,
    },

    /// 任意 channel 取数收束(成功 / 失败 / 取消都发)。纯埋点信号(fetches),与具体
    /// 结果(`SearchResults` 等事件)并行——server 记录后不转发 client。
    FetchDone {
        /// 取数种类。
        kind: ChannelFetchKindTag,

        /// 来源。
        source: SourceKind,

        /// 目标 qualified 引用;只有 source 的形态(my_playlists / search)无目标为 `None`。
        target_ref: Option<String>,

        /// 是否 user 优先级触发(否则系统链路,如预取 / 回填)。
        from_user: bool,

        /// 收束结局。
        outcome: TaskOutcome,

        /// 耗时 ms。
        latency_ms: u64,
    },

    /// 某来源的一份完整收藏专辑列表；缓存与远端结果分别交给服务端合并。
    AlbumsFetched {
        /// 音乐来源。
        source: SourceKind,

        /// 来源内的收藏顺序。
        albums: Vec<Album>,
    },

    /// 各来源收藏专辑的完整快照，客户端按此替换列表。
    SavedAlbumsSnapshot {
        /// 按来源注册顺序合并的收藏专辑。
        albums: Vec<Album>,
    },

    /// 专辑详情请求失败或取消，客户端释放在途状态。
    AlbumDetailFailed {
        /// 请求的专辑身份。
        id: AlbumId,
    },
}

impl TaskEvent {
    /// Whether this buffered result belongs to the invalidated category or parent.
    pub fn affected_by(&self, scope: &RefreshScope) -> bool {
        match self {
            Self::PlaylistsFetched { source, .. } => {
                ChannelFetchKind::MyPlaylists { source: *source }.affected_by(scope)
            }
            Self::AlbumsFetched { source, .. } => {
                ChannelFetchKind::MyAlbums { source: *source }.affected_by(scope)
            }
            Self::PlaylistDetailFetched { id, load, .. }
            | Self::PlaylistDetailFailed { id, load } => ChannelFetchKind::PlaylistDetail {
                id: id.clone(),
                load: *load,
            }
            .affected_by(scope),
            Self::AlbumDetailFetched { id, .. } | Self::AlbumDetailFailed { id } => {
                ChannelFetchKind::AlbumDetail { id: id.clone() }.affected_by(scope)
            }
            Self::LibrarySnapshot { .. } => {
                *scope == RefreshScope::Collections(CollectionCategory::Playlists)
            }
            Self::SavedAlbumsSnapshot { .. } => {
                *scope == RefreshScope::Collections(CollectionCategory::Albums)
            }
            _ => false,
        }
    }
}

/// 搜索结果载荷,变体与请求的 [`SearchKind`] 一致。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SearchPayload {
    /// 歌曲结果。
    Songs(Vec<Song>),

    /// 专辑结果(曲目留空)。
    Albums(Vec<Album>),

    /// 歌单结果(曲目留空)。
    Playlists(Vec<Playlist>),

    /// artist 结果(热门曲留空)。
    Artists(Vec<Artist>),
}
