//! daemon 音乐脚本的命令、查询结果与回调请求。
//!
//! Lua 值只存在于 API 与回调边界；线程之间传递结构化音乐模型和回执。

use mineral_model::{BitRate, PlaylistEntry, Song, SongId};
use mineral_protocol::PlayMode;

/// 脚本发往 daemon 的音乐命令与 session 配置覆盖。
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptCmd {
    /// 播放 / 暂停切换。
    Toggle,

    /// 下一首。
    Next,

    /// 上一首。
    Prev,

    /// 停止播放。
    Stop,

    /// 相对 seek(秒,可负)。
    SeekRel(f64),

    /// 绝对 seek(秒)。
    SeekTo(f64),

    /// 设音量(0..=100)。
    SetVolume(u8),

    /// 设播放模式。
    SetMode(PlayMode),

    /// 播放指定歌曲。
    Play(SongId),

    /// 下载指定歌曲。
    Download(SongId),

    /// 读 per-song 持久值;结果以 [`ResolveValue::Store`] 回投 `query`。
    StoreGet {
        /// 目标歌。
        song: SongId,

        /// 开放键。
        key: String,

        /// 结果回投句柄。
        query: QueryId,
    },

    /// 写 per-song 持久值(`Nil` 删除)。fire-and-forget,失败只记日志。
    StoreSet {
        /// 目标歌。
        song: SongId,

        /// 开放键。
        key: String,

        /// 标量值。
        value: mineral_protocol::StoreValue,
    },

    /// 读当前播放队列;结果以 [`ResolveValue::Songs`] 回投 `query`。
    QueueList {
        /// 结果回投句柄。
        query: QueryId,
    },

    /// 整表重排队列。id 必须来自当前队列，出现次数不限；混入外来 id 则整体被拒。
    QueueSet {
        /// 新的队列顺序。
        ids: Vec<SongId>,
    },

    /// 读用户歌单列表;结果以 [`ResolveValue::Playlists`] 回投 `query`。
    LibraryPlaylists {
        /// 结果回投句柄。
        query: QueryId,
    },

    /// 读指定歌单的 membership relation。
    LibraryTracks {
        /// 目标歌单。
        playlist: mineral_model::PlaylistId,

        /// 结果以 [`ResolveValue::PlaylistEntries`] 回投此句柄。
        query: QueryId,
    },

    /// 按关键词搜索歌曲;结果以 [`ResolveValue::Songs`] 回投 `query`。
    LibrarySearch {
        /// 搜索关键词。
        term: String,

        /// 限定来源；`None` 跨全部来源聚合，单个 channel 失败跳过。
        source: Option<mineral_model::SourceKind>,

        /// 起始偏移(从 0 起)。
        offset: u32,

        /// 单页返回上限。
        limit: u32,

        /// 结果回投句柄。
        query: QueryId,
    },

    /// 解析歌曲的 direct media；没有直链时回投查询错误。
    LibrarySongUrl {
        /// 目标歌，namespace 决定 playback provider。
        song: SongId,

        /// 结果以 [`ResolveValue::DirectMedia`] 回投此句柄。
        query: QueryId,
    },

    /// 设 / 取消一首歌的 love。fire-and-forget,失败只记日志。
    SetLoved {
        /// 目标歌。
        song: SongId,

        /// true=喜欢,false=取消。
        loved: bool,
    },

    /// 一批叶子覆盖原子应用于 daemon 私有配置。
    ConfigOverride {
        /// 待应用的叶子覆盖。
        ops: Vec<ConfigOverrideOp>,
    },
}

/// 一条配置覆盖叶子 op。宿主深合并后落型校验；无效叶子按路径剔除。
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigOverrideOp {
    /// 宿主配置中的叶子路径；daemon 与 TUI 分别校验自己的配置。
    pub path: String,

    /// 覆盖值；`None` 撤销。Lua nil 收敛成 `None`，不产生 `Some(Nil)`。
    pub value: Option<mineral_protocol::BusValue>,
}

/// 音乐查询的回投句柄，由脚本注册回调后随查询命令发给 daemon。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QueryId(pub(crate) u64);

/// daemon 回投的音乐查询结果；失败时 Lua 回调接收 `(nil, err)`。
#[derive(Debug)]
pub enum ResolveValue {
    /// per-song 持久值(`store.get`)。
    Store(mineral_protocol::StoreValue),

    /// 无 collection membership 的歌曲列表(`queue.list` / `library.search`)。
    Songs(Vec<Song>),

    /// 歌单 membership 列表，每项保留 0-based canonical index。
    PlaylistEntries(Vec<PlaylistEntry>),

    /// 歌单列表(`library.playlists`)。
    Playlists(Vec<PlaylistBrief>),

    /// `library.song_url` 解析的媒体与请求档位。
    DirectMedia {
        /// Playback provider 的播放资源和媒体信息。
        media: Box<mineral_model::DirectMedia>,

        /// 请求档位，不是实测音质。
        requested_quality: BitRate,
    },

    /// 查询失败；跨线程保留原始错误，到 Lua 回调边界才渲染错误链。
    Error(Box<dyn std::error::Error + Send + Sync>),
}

/// 歌单轻量投影，供 `library.playlists` 与 curate transform 共用。
#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistBrief {
    /// 歌单 id(Lua 侧用 `qualified()` 字符串)。
    pub id: mineral_model::PlaylistId,

    /// 歌单名。
    pub name: String,

    /// 曲目数。
    pub track_count: u64,

    /// 简介，与 [`mineral_model::Playlist`] 同约定。
    pub description: String,

    /// 播放量;拿不到为 `None`(Lua 侧缺席为 nil)。
    pub play_count: Option<u64>,

    /// 收藏 / 订阅数;拿不到为 `None`(Lua 侧缺席为 nil)。
    pub subscriber_count: Option<u64>,
}

impl From<&mineral_model::Playlist> for PlaylistBrief {
    fn from(playlist: &mineral_model::Playlist) -> Self {
        Self {
            id: playlist.id.clone(),
            name: playlist.name.clone(),
            track_count: playlist.track_count,
            description: playlist.description.clone(),
            play_count: playlist.play_count,
            subscriber_count: playlist.subscriber_count,
        }
    }
}

/// curate transform 采纳的歌单条目；daemon 对回真实歌单，未知 id 丢弃、重复取首见。
#[derive(Clone, Debug, PartialEq)]
pub struct CuratedEntry {
    /// 目标歌单(qualified id 解析回来)。
    pub id: mineral_model::PlaylistId,

    /// 展示名覆盖;`None` = 保留原名。
    pub name: Option<String>,

    /// 简介覆盖;`None` = 保留原文。
    pub description: Option<String>,
}

/// curate transform 的采纳结果。
#[derive(Clone, Debug, PartialEq)]
pub enum CurateOutcome {
    /// 无函数、执行失败或超时，原列表透传，歌单不因脚本错误消失。
    Identity,

    /// 省略条目即隐藏，列表顺序即展示顺序，允许覆盖名称与简介。
    Curated(Vec<CuratedEntry>),
}

/// daemon 脚本线程消费的查询回投、音乐拦截、配置回调或停机请求。
#[derive(Debug)]
pub(crate) enum ScriptMsg {
    /// 一次异步查询的结果回投。
    Resolve {
        /// 对应在途表里的 Lua 回调。
        query: QueryId,

        /// 查询结果。
        value: ResolveValue,
    },

    /// 同步拦截 `before_stream`，daemon 带墙钟超时等待裁决。
    InterceptStream {
        /// 入参快照。
        ctx: crate::hooks::BeforeStreamCtx,

        /// 裁决回执，接收端超时放弃时静默丢。
        reply: tokio::sync::oneshot::Sender<crate::hooks::HookDecision>,
    },

    /// 同步拦截 `before_download`。
    InterceptDownload {
        /// 入参快照。
        ctx: crate::hooks::BeforeDownloadCtx,

        /// 裁决回执，接收端超时放弃时静默丢。
        reply: tokio::sync::oneshot::Sender<crate::hooks::HookDecision>,
    },

    /// 执行 config 中的 curate transform。
    CuratePlaylists {
        /// `Some` 取来源函数；`None` 取跨来源的合并函数。
        source: Option<mineral_model::SourceKind>,

        /// 按来源的全量歌单或跨来源合并歌单。
        briefs: Vec<PlaylistBrief>,

        /// 采纳结果回执。
        reply: tokio::sync::oneshot::Sender<CurateOutcome>,
    },

    /// 拉取注册了 curate 函数的来源名，供 daemon 校验 channel 配置。
    GetCurateKeys {
        /// 来源名回执。
        reply: tokio::sync::oneshot::Sender<Vec<String>>,
    },

    /// 执行 config 的队列变换函数。
    QueueTransform {
        /// daemon 配置中的唯一操作名。
        name: String,

        /// 当前有序队列。
        queue: Vec<Song>,

        /// 在播条目的 0-based 下标。
        current: usize,

        /// 光标的 0-based 下标，无光标时缺席。
        selected: Option<usize>,

        /// 新顺序的 id 序列回执。
        reply: tokio::sync::oneshot::Sender<crate::Result<Vec<SongId>>>,
    },

    /// 优雅停机，退出主循环。
    Stop,
}
