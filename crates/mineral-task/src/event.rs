//! 任务推到 client 的事件载荷。

use mineral_channel_core::Page;
use mineral_model::{
    Album, Artist, ArtistId, Lyrics, PlayUrl, Playlist, PlaylistId, SearchKind, Song, SongId,
    SourceKind,
};
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};

use crate::write::{PlaylistWriteOp, WriteError};

/// 任务完成时,channel 中央事件 buffer 推给 client 消费的载荷。
///
/// 失败任务不发 event(只在 [`crate::TaskHandle::done`] 上拿到 [`crate::TaskOutcome::Failed`]),
/// 详细错误进 mineral-log。**例外:[`TaskEvent::PlaylistWriteDone`] 失败也发**——
/// 写操作的失败必须到达用户(toast + 清 pending 标记),不能只留在日志里。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TaskEvent {
    /// `MyPlaylists` 任务成功:某 channel 当前用户的歌单列表已到。
    PlaylistsFetched {
        /// 来源 channel。
        source: SourceKind,

        /// 拉到的歌单。
        playlists: Vec<Playlist>,
    },

    /// 用户歌单库合并快照:server 聚合各源列表(经出口变换)后的整表,
    /// **全量替换**语义,顺序即展示序。非任务事件(server 聚合态直接推,
    /// 不经 channel-fetch lane);[`TaskEvent::PlaylistsFetched`] 是它的
    /// server 内部原料,不再直达 client。
    LibrarySnapshot {
        /// 合并后的歌单列表(展示序)。
        playlists: Vec<Playlist>,
    },

    /// 某源 canonical 收藏(♥)集已到:server 从本地 persist 读、经远端红心导入合并后推送,
    /// client 据此装饰 SongView。非任务事件(server 直接推,不经 channel-fetch lane)。
    LikedSongIdsFetched {
        /// 来源 channel。
        source: SourceKind,

        /// 已收藏的歌曲 ID 集合(canonical,以本地 persist 为准)。
        ids: FxHashSet<SongId>,
    },

    /// `PlaylistDetail` 任务成功:歌单完整详情(元信息 + 曲目)已到。
    PlaylistDetailFetched {
        /// 歌单 id。
        id: PlaylistId,

        /// 完整歌单(含曲目)。
        playlist: Box<Playlist>,
    },

    /// `SongUrl` 任务成功:可播放 URL 解析就绪。
    PlayUrlReady {
        /// 关联的歌曲 id。
        song_id: SongId,

        /// 解析出的播放 URL + 元信息。
        play_url: PlayUrl,
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

    /// `Search` 任务成功:一页搜索结果已到。
    ///
    /// 回带完整请求四元组,client 据此配对到对应搜索会话;query 已变的
    /// 过期响应由 client 直接丢弃。
    SearchResults {
        /// 来源 channel。
        source: SourceKind,

        /// 搜索实体类型。
        kind: SearchKind,

        /// 关键词。
        query: String,

        /// 分页参数。
        page: Page,

        /// 结果载荷(变体与 `kind` 一致)。
        payload: SearchPayload,

        /// channel 的显式翻页信号(`SearchHits::has_more` 透传):`None` = 源不知道,
        /// client 回退「返回条数 < limit 即榨干」推断。
        #[serde(default)]
        has_more: Option<bool>,
    },

    /// `ArtistDetail` 任务成功:歌手简介 + 热门曲目已到。
    ArtistDetailFetched {
        /// 歌手 id。
        id: ArtistId,

        /// 歌手详情。
        artist: Box<Artist>,
    },

    /// `ArtistAlbums` 任务成功:歌手的一页专辑列表已到。
    ArtistAlbumsFetched {
        /// 歌手 id。
        id: ArtistId,

        /// 分页参数(client 配对翻页用)。
        page: Page,

        /// 专辑列表(曲目留空)。
        albums: Vec<Album>,
    },

    /// `AlbumDetail` 任务成功:专辑完整详情(元信息 + 曲目)已到。
    AlbumDetailFetched {
        /// 专辑 id。
        id: mineral_model::AlbumId,

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
}

/// 搜索结果载荷,变体与请求的 [`SearchKind`] 一致。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SearchPayload {
    /// 歌曲结果。
    Songs(Vec<Song>),

    /// 专辑结果(曲目留空)。
    Albums(Vec<Album>),

    /// 歌单结果(曲目留空)。
    Playlists(Vec<Playlist>),

    /// 歌手结果(热门曲留空)。
    Artists(Vec<Artist>),
}
