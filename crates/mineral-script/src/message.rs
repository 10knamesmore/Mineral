//! daemon ↔ 脚本线程之间的内部消息类型。
//!
//! 方向约定:[`ScriptEvent`] 是 daemon → 脚本(投递给 Lua 回调的事件),
//! [`ScriptCmd`] 是脚本 → daemon(Lua API 发出的播放器命令)。两侧都是
//! **结构化** Rust 类型,Lua 字符串只出现在 VM 边界的适配层(`api` 模块)。

use mineral_model::{Song, SongId};
use mineral_protocol::PlayMode;

/// daemon 投递给脚本线程的事件。携带 daemon 侧已有的完整模型
/// (如整个 [`Song`]),投影成 Lua table 的裁剪发生在 dispatch 层。
#[derive(Clone, Debug)]
pub enum ScriptEvent {
    /// 在播曲目变更(与 `player.song` 属性同源:远端起播 / 本地命中 /
    /// gapless 推进全覆盖;同曲重启不重复触发)。
    TrackStarted {
        /// 开始播放的歌曲。
        song: Box<Song>,
    },

    /// 一首歌结束(必带 reason,与 wire 的 `FinishReason` 同构)。
    TrackFinished {
        /// 结束的歌曲。
        song: Box<Song>,

        /// 结束原因。
        reason: TrackFinishedReason,
    },

    /// 一首歌下载完成(永久导出落盘;已存在跳过不触发)。
    DownloadCompleted {
        /// 下载完成的歌曲。
        song: Box<Song>,

        /// 落盘路径。
        path: std::path::PathBuf,

        /// 实际下载音质(hook 改写后的有效值)。
        quality: mineral_model::BitRate,

        /// 容器格式(channel 实际提供;拿不到为 `None`,Lua 侧投影成 nil)。
        format: Option<mineral_model::AudioFormat>,
    },

    /// 属性树某项变更(PR-3 接 `mineral.observe` 后真正消费;变体先定形)。
    PropertyChanged {
        /// 属性键。
        key: PropKey,

        /// 新值。
        value: PropValue,
    },
}

/// 曲目结束原因(内部表示,与 `mineral_protocol::FinishReason` 同构;
/// 不直接复用是为了脚本层不感知 wire 演进)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackFinishedReason {
    /// 自然播完。
    Eof,

    /// 用户跳过(next / prev 切歌)。
    Skip,

    /// 解码 / 取链失败导致中断。
    Error,

    /// 用户显式停止。
    Stop,
}

impl TrackFinishedReason {
    /// 给 Lua 回调的字符串表示。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eof => "eof",
            Self::Skip => "skip",
            Self::Error => "error",
            Self::Stop => "stop",
        }
    }
}

/// 可观测属性键(**封闭**枚举,与 `mineral_protocol::PropName` 的六个内置常量
/// 一一对应)。protocol 侧 `PropName` 为前向兼容保持开放;脚本侧 observe 必须
/// 校验合法名,故这里收成封闭集合。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PropKey {
    /// 当前在播歌(qualified id 字符串;无在播为 none)。
    PlayerSong,

    /// 播放态("playing" / "paused" / "stopped")。
    PlayerState,

    /// 音量百分比(0..=100)。
    PlayerVolume,

    /// 播放进度(整秒)。
    PlayerPosition,

    /// 播放模式(`PlayMode` 稳定名)。
    PlayerMode,

    /// 队列长度。
    QueueLength,

    /// 终端 UI 状态(复合属性:rows/cols/fullscreen;无 client 在线为 none)。
    Terminal,
}

impl PropKey {
    /// 全部属性键(`mineral.observe` 错误信息 / meta 守卫测试用)。
    pub const ALL: [Self; 7] = [
        Self::PlayerSong,
        Self::PlayerState,
        Self::PlayerVolume,
        Self::PlayerPosition,
        Self::PlayerMode,
        Self::QueueLength,
        Self::Terminal,
    ];

    /// 按属性名解析(与 [`Self::as_str`] 对偶);未知名为 `None`。
    ///
    /// # Params:
    ///   - `name`: 属性名字符串(脚本侧输入)
    ///
    /// # Return:
    ///   对应键;未知名为 `None`,调用方报脚本错误。
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "player.song" => Some(Self::PlayerSong),
            "player.state" => Some(Self::PlayerState),
            "player.volume" => Some(Self::PlayerVolume),
            "player.position" => Some(Self::PlayerPosition),
            "player.mode" => Some(Self::PlayerMode),
            "queue.length" => Some(Self::QueueLength),
            "terminal" => Some(Self::Terminal),
            _ => None,
        }
    }

    /// 属性名字符串(与 `PropName` 的内置常量字面量一致)。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlayerSong => "player.song",
            Self::PlayerState => "player.state",
            Self::PlayerVolume => "player.volume",
            Self::PlayerPosition => "player.position",
            Self::PlayerMode => "player.mode",
            Self::QueueLength => "queue.length",
            Self::Terminal => "terminal",
        }
    }
}

/// 属性值(内部表示)。与 `mineral_protocol::PropValue` 同构但独立定形,
/// 理由同 [`TrackFinishedReason`]。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropValue {
    /// 布尔(terminal 的 fullscreen 字段)。
    Bool(bool),

    /// 整数(volume / position 整秒 / queue.length)。
    Int(i64),

    /// 字符串(state / mode 名 / song 的 qualified id)。
    Str(String),

    /// 复合结构(有序键值对,如 `terminal`)。递归用自身而非 BusValue:
    /// BusValue 带 Float 无 `Eq`,会破坏属性缓存的 diff / 回放语义。
    Table(Vec<(String, Self)>),

    /// 缺省 / 空(如无在播歌)。
    None,
}

/// 脚本发往 daemon 的播放器命令。daemon 侧由独立 task drain 并落到
/// player / download 执行面(PR-4 接线)。
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

    /// per-song 数值自增;带 `query` 时回投自增后的值。
    StoreInc {
        /// 目标歌。
        song: SongId,

        /// 开放键。
        key: String,

        /// 增量(可负)。
        delta: i64,

        /// 结果回投句柄(脚本侧没传回调则为 `None`,失败只记日志)。
        query: Option<QueryId>,
    },

    /// 读当前播放队列;结果以 [`ResolveValue::Songs`] 回投 `query`。
    QueueList {
        /// 结果回投句柄。
        query: QueryId,
    },

    /// 读用户歌单列表;结果以 [`ResolveValue::Playlists`] 回投 `query`。
    LibraryPlaylists {
        /// 结果回投句柄。
        query: QueryId,
    },

    /// 读指定歌单的曲目;结果以 [`ResolveValue::Songs`] 回投 `query`。
    LibraryTracks {
        /// 目标歌单。
        playlist: mineral_model::PlaylistId,

        /// 结果回投句柄。
        query: QueryId,
    },

    /// 按关键词搜索歌曲;结果以 [`ResolveValue::Songs`] 回投 `query`。
    LibrarySearch {
        /// 搜索关键词。
        term: String,

        /// 限定源;`None` = 跨全部源聚合(单源失败跳过该源)。
        source: Option<mineral_model::SourceKind>,

        /// 起始偏移(从 0 起)。
        offset: u32,

        /// 单页返回上限。
        limit: u32,

        /// 结果回投句柄。
        query: QueryId,
    },

    /// 解析一首歌的可播 URL(按 id 的 namespace 找 channel 取流);结果以
    /// [`ResolveValue::PlayUrl`] 回投 `query`,无可播资源时回错误。
    LibrarySongUrl {
        /// 目标歌(namespace 决定走哪个 channel)。
        song: SongId,

        /// 结果回投句柄。
        query: QueryId,
    },

    /// 设/取消一首歌的 love。fire-and-forget,失败只记日志。
    SetLoved {
        /// 目标歌。
        song: SongId,

        /// true=喜欢,false=取消。
        loved: bool,
    },

    /// 起一个子进程;结束后以 [`ResolveValue::Spawn`] 回投 `query`。
    Spawn {
        /// 脚本侧分配的标识(`handle:kill()` 经它路由)。
        id: crate::proc::SpawnId,

        /// 结构化参数。
        spec: crate::proc::SpawnSpec,

        /// 结果回投句柄。
        query: QueryId,
    },

    /// 中止一个在跑的子进程(已退出 / 未知 id 为 no-op)。
    SpawnKill {
        /// 目标子进程标识。
        id: crate::proc::SpawnId,
    },

    /// session 级 UI 旋钮覆盖(`mineral.ui.override`)。daemon 零解释:
    /// 记 opaque 表 + 转发 [`Event::UiOverride`](mineral_protocol::Event::UiOverride),
    /// key→旋钮映射在 client 边缘。
    UiOverride {
        /// 旋钮键(约定 = 配置路径,如 `lyrics.fullscreen_line_gap`)。
        key: String,

        /// 覆盖值;`None` = 撤销(Lua 侧传 nil)。`Some(Nil)` 不出现,
        /// API 层把 nil 收敛成 `None`,避免「覆盖成 Nil」与「撤销」两义。
        value: Option<mineral_protocol::BusValue>,
    },
}

/// 一次异步查询的回投句柄:脚本侧把 Lua 回调挂进 pending 表拿到它,
/// daemon 泵完成查询后凭它回投结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct QueryId(pub(crate) u64);

/// 异步查询的结果(结构化;Lua 值的转换在脚本线程的 dispatch 层)。
///
/// 失败统一走 [`Self::Error`],Lua 回调收 `(nil, err)`。
#[derive(Clone, Debug, PartialEq)]
pub enum ResolveValue {
    /// per-song 持久值(`store.get` / `store.inc`)。
    Store(mineral_protocol::StoreValue),

    /// 歌曲列表(`queue.list` / `library.tracks`),按序投影成 Lua 数组。
    Songs(Vec<Song>),

    /// 歌单列表(`library.playlists`)。
    Playlists(Vec<PlaylistBrief>),

    /// 一首歌的可播 URL + 元信息(`library.song_url`),投影含取流头/布局,
    /// 可直接回填 hook 的改写返回值。
    PlayUrl(Box<mineral_model::PlayUrl>),

    /// 子进程结束(`mineral.spawn` 回调)。
    Spawn(crate::proc::SpawnResult),

    /// 查询失败(人读信息)。
    Error(String),
}

/// 歌单在脚本侧的轻量投影(不携带曲目,曲目另经 `library.tracks` 拉)。
///
/// `library.playlists` 回调与 curate transform 入参共用这一投影。
#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistBrief {
    /// 歌单 id(Lua 侧用 `qualified()` 字符串)。
    pub id: mineral_model::PlaylistId,

    /// 歌单名。
    pub name: String,

    /// 曲目数。
    pub track_count: u64,

    /// 简介(拿不到为空串,与 [`mineral_model::Playlist`] 同约定)。
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

/// curate transform 采纳的一条歌单条目。daemon 侧凭 `id` 对回真实
/// `Playlist`(未知 id 丢弃、重复取首见,在 daemon 落地)。
#[derive(Clone, Debug, PartialEq)]
pub struct CuratedEntry {
    /// 目标歌单(qualified id 解析回来)。
    pub id: mineral_model::PlaylistId,

    /// 展示名覆盖;`None` = 保留原名。
    pub name: Option<String>,

    /// 简介覆盖;`None` = 保留原文。
    pub description: Option<String>,
}

/// 一次 curate 往返的结果。
#[derive(Clone, Debug, PartialEq)]
pub enum CurateOutcome {
    /// 原列表透传:无函数注册(常态,非错误)/ 函数失败 / 超时(fail-open,
    /// 歌单不因脚本 bug 消失)。
    Identity,

    /// transform 采纳:省略 = 隐藏,顺序 = 展示序,`name`/`description` 可覆盖。
    Curated(Vec<CuratedEntry>),
}

/// 脚本线程主循环消费的信封:事件投递、动作调用或停机。
#[derive(Debug)]
pub(crate) enum ScriptMsg {
    /// 投递一个事件给已注册的 Lua 回调。
    Event(ScriptEvent),

    /// 调用一个具名动作(`mineral.action` 注册),结果经 oneshot 回执。
    Action {
        /// 动作注册名。
        name: String,

        /// 按键瞬间的 client 上下文(无界面触发面为 `None`,回调收空表)。
        ctx: Option<mineral_protocol::KeyContext>,

        /// 调用位置实参(CLI `mineral action <name> <args...>` 采集;
        /// TUI 键位 / 无参触发为空)。Lua 回调经 `ctx.args` 读取(恒为数组)。
        args: Vec<String>,

        /// 调用结果回执(接收端 drop 时静默丢)。
        reply: tokio::sync::oneshot::Sender<ActionOutcome>,
    },

    /// 一次异步查询的结果回投(daemon 泵完成 [`ScriptCmd`] 查询后发回)。
    Resolve {
        /// 查询句柄(对应 pending 表里的 Lua 回调)。
        query: QueryId,

        /// 查询结果。
        value: ResolveValue,
    },

    /// 拉取 `mineral.bind` 的键绑定表(daemon 处理 `Request::ScriptBinds` 用)。
    GetBinds {
        /// bind 表回执(接收端 drop 时静默丢)。
        reply: tokio::sync::oneshot::Sender<Vec<mineral_protocol::ScriptBind>>,
    },

    /// 同步拦截 `before_stream`:跑回调链并回执裁决(daemon 侧带墙钟超时 await)。
    InterceptStream {
        /// 入参快照。
        ctx: crate::hooks::BeforeStreamCtx,

        /// 裁决回执(接收端超时放弃时静默丢)。
        reply: tokio::sync::oneshot::Sender<crate::hooks::HookDecision>,
    },

    /// 同步拦截 `before_download`:跑回调链并回执裁决(daemon 侧带墙钟超时 await)。
    InterceptDownload {
        /// 入参快照。
        ctx: crate::hooks::BeforeDownloadCtx,

        /// 裁决回执(接收端超时放弃时静默丢)。
        reply: tokio::sync::oneshot::Sender<crate::hooks::HookDecision>,
    },

    /// 跑一级 curate transform(config `sources` 摘出的函数),回执采纳结果。
    CuratePlaylists {
        /// `Some` = per-source 函数(按源名取);`None` = 跨源函数(合并列表)。
        source: Option<mineral_model::SourceKind>,

        /// 待 transform 的歌单投影(per-source 给该源全量,跨源给合并列表)。
        briefs: Vec<PlaylistBrief>,

        /// 采纳结果回执(接收端超时放弃时静默丢)。
        reply: tokio::sync::oneshot::Sender<CurateOutcome>,
    },

    /// 拉取 per-source curate 函数的源名键集(daemon 对无对应 channel 的键
    /// 打 warn 用),回执经 oneshot。
    GetCurateKeys {
        /// 键集回执(接收端 drop 时静默丢)。
        reply: tokio::sync::oneshot::Sender<Vec<String>>,
    },

    /// 渲染一个复制模板(config `copy.templates[index]` 的函数),结果经
    /// oneshot 回执(daemon 处理 `Request::RenderCopyTemplate` 用)。
    RenderCopyTemplate {
        /// 模板下标(0-based,对位 config 数组序)。
        index: usize,

        /// 模板作用的实体(client 随请求带来)。
        ctx: mineral_protocol::CopyTemplateCtx,

        /// 渲染结果回执(接收端 drop 时静默丢)。
        reply: tokio::sync::oneshot::Sender<Result<String, String>>,
    },

    /// 优雅停机:主循环退出,线程结束。
    Stop,
}

/// 一次具名动作调用的结果。
#[derive(Debug, PartialEq, Eq)]
pub enum ActionOutcome {
    /// 回调执行完成。
    Done,

    /// 该名字未注册。
    NotFound,

    /// 回调执行失败(Lua 错误 / 超看门狗硬阈值被中断),携带单行错误信息。
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::TrackFinishedReason;

    #[test]
    fn meta_stub_finish_reason_alias_matches_rust() -> color_eyre::Result<()> {
        use color_eyre::eyre::WrapErr;
        // meta/mineral.lua 的 `mineral.FinishReason` 字符串枚举必须与
        // Rust 侧 `as_str` 的全部取值逐字一致(顺序也钉死)。
        let meta_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../mineral-config/src/lua/meta/mineral.lua"
        );
        let meta = std::fs::read_to_string(meta_path).wrap_err("read meta/mineral.lua")?;
        let literals = [
            TrackFinishedReason::Eof,
            TrackFinishedReason::Skip,
            TrackFinishedReason::Error,
            TrackFinishedReason::Stop,
        ]
        .map(|reason| format!("\"{}\"", reason.as_str()))
        .join("|");
        let alias = format!("---@alias mineral.FinishReason {literals}");
        assert!(
            meta.contains(&alias),
            "meta stub 缺少与 Rust 一致的别名行:`{alias}`"
        );
        Ok(())
    }

    #[test]
    fn meta_stub_view_kind_alias_matches_rust() -> color_eyre::Result<()> {
        use color_eyre::eyre::WrapErr;
        use mineral_protocol::ViewKind;
        // meta/mineral.lua 的 `mineral.ViewKind` 字符串枚举必须与
        // Rust 侧 `script_name` 的全部取值逐字一致(顺序也钉死)。
        let meta_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../mineral-config/src/lua/meta/mineral.lua"
        );
        let meta = std::fs::read_to_string(meta_path).wrap_err("read meta/mineral.lua")?;
        let literals = [
            ViewKind::Playlists,
            ViewKind::Tracks,
            ViewKind::Queue,
            ViewKind::Fullscreen,
            ViewKind::Search,
        ]
        .map(|view| format!("\"{}\"", view.script_name()))
        .join("|");
        let alias = format!("---@alias mineral.ViewKind {literals}");
        assert!(
            meta.contains(&alias),
            "meta stub 缺少与 Rust 一致的别名行:`{alias}`"
        );
        Ok(())
    }
}
