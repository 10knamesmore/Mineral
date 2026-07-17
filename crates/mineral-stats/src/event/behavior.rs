//! 行为域事件:有人 / 脚本发起、统一带 actor 的交互流水。
//!
//! 每个变体一一对应一张强 schema 事件表,变体的具名字段 = 该表除公共列
//! (id / ts / session_id / actor)外的专有列。取值有限、进 SQL CHECK 的列在此用
//! `sqlx::Type` 小枚举表示,写时直接 bind、读时 `as "col: T"` decode。

use mineral_model::{PlaylistId, SongId, SourceKind};

use crate::vocab::PlayMode;

/// 搜索词的稳定散列(FNV-1a 64-bit,十六进制)。手写算法故跨进程 / 版本稳定
/// (不用 std 随机化 hasher),供「保次数 / 去重」——同词恒同散列,散列档丢原文只留它。
///
/// # Params:
///   - `query`: 搜索词原文
///
/// # Return:
///   16 位十六进制散列串
pub fn query_hash(query: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64; // FNV offset basis
    for byte in query.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3); // FNV prime
    }
    format!("{hash:016x}")
}

/// 搜索目标类型(searches.kind)。
///
/// 是领域 `SearchKind` 的落库子集——埋点只记这四类实体搜索,不含用户搜索。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum SearchTargetKind {
    /// 单曲搜索。
    Song,

    /// 专辑搜索。
    Album,

    /// 艺人搜索。
    Artist,

    /// 歌单搜索。
    Playlist,
}

/// 一次搜索的结局(searches.outcome)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum SearchOutcome {
    /// 正常返回结果。
    Ok,

    /// 后端报错。
    Failed,

    /// 被更新的搜索取代 / 用户放弃。
    Cancelled,
}

/// 取数种类(fetches.fetch_kind)。
///
/// 与 server 侧取数任务种类同构但独立定义(stats 不依赖 task 层),边界转换在 server;
/// 落库串与既有 snake_case 词汇一致。搜索 / 取链除各自专表外也在 fetches 记一行收束
/// (专表富化,fetches 是取数全谱)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum FetchKind {
    /// 我的歌单列表。
    MyPlaylists,

    /// 歌单详情。
    PlaylistDetail,

    /// 播放直链。
    SongUrl,

    /// 歌词。
    Lyrics,

    /// 远端播放数。
    RemotePlayCount,

    /// 实体搜索。
    Search,

    /// 艺人详情。
    ArtistDetail,

    /// 艺人专辑列表。
    ArtistAlbums,

    /// 专辑详情。
    AlbumDetail,
}

/// 歌单写操作类型(playlist_ops.op)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum PlaylistOpKind {
    /// 新建歌单。
    Create,

    /// 删除歌单。
    Delete,

    /// 添加歌曲。
    Add,

    /// 移除歌曲。
    Remove,

    /// 重命名。
    Rename,

    /// 改描述。
    SetDescription,
}

/// 歌单写操作的目标引用(playlist_ops.playlist_ref 单列落库)。
///
/// 已存在的歌单有结构化 id;新建(create)时歌单尚无 id,只有来源 + 拟用名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaylistRef {
    /// 已存在的歌单。
    Existing(PlaylistId),

    /// 新建中的歌单(尚无 id):来源 + 拟用名。
    Creating {
        /// 目标来源。
        source: SourceKind,

        /// 拟用歌单名。
        name: String,
    },
}

impl PlaylistRef {
    /// 落库串:已存在为 `qualified()`(`namespace:value`),新建为 `source:名`(同形,
    /// 全局可读)。
    ///
    /// # Return:
    ///   playlist_ref 列值
    pub fn to_column(&self) -> String {
        match self {
            Self::Existing(id) => id.qualified(),
            Self::Creating { source, name } => format!("{}:{name}", source.name()),
        }
    }
}

/// 暂停 / 恢复动作(pauses.action)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum PauseAction {
    /// 暂停播放。
    Pause,

    /// 恢复播放。
    Resume,
}

/// 一次收藏变更的发起语义(love_changes.origin)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum LoveOrigin {
    /// 用户显式操作。
    User,

    /// 导入 / 聚合旁路(非用户当场按)。
    Import,
}

/// 收藏变更向远端镜像的结果(love_changes.remote_mirror)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum RemoteMirror {
    /// 远端同步成功。
    Ok,

    /// 该来源不支持远端收藏。
    NotSupported,

    /// 远端同步失败。
    Failed,
}

/// 队列写操作类型(queue_ops.op)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum QueueOp {
    /// 整队替换。
    Set,

    /// 插到下一首。
    InsertNext,

    /// 追加到队尾。
    Append,

    /// 清空队列。
    Clear,

    /// 移除某条。
    Remove,
}

/// 二值操作结局(playlist_ops / copy_renders / action_invocations 的 outcome)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum OpOutcome {
    /// 操作成功。
    Ok,

    /// 操作失败。
    Failed,
}

/// 歌单写操作的失败归类(playlist_ops.error_kind)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum PlaylistError {
    /// 需要登录。
    AuthRequired,

    /// 被限流。
    RateLimited,

    /// 该来源不支持此写操作。
    NotSupported,

    /// 后端 API 报错。
    Api,

    /// 其他错误。
    Other,
}

/// 取数的触发方(fetches.trigger)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum FetchTrigger {
    /// 用户操作触发。
    User,

    /// 系统链路触发(预取 / 补回填等)。
    System,
}

/// 一次取数的结局(fetches.outcome)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum FetchOutcome {
    /// 正常返回。
    Ok,

    /// 取数失败。
    Failed,

    /// 被取代 / 放弃。
    Cancelled,
}

/// 一次下载的结局(downloads.outcome)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum DownloadOutcome {
    /// 完成下载。
    Downloaded,

    /// 已存在等原因跳过。
    Skipped,

    /// 下载失败。
    Failed,
}

/// 下载路径上插件顶换的结果(downloads.hooked)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum DownloadHook {
    /// 未介入。
    None,

    /// 改写了 URL。
    Rewrite,

    /// 顶掉本次下载。
    Skip,
}

/// 渲染文案的上下文类型(copy_renders.ctx_kind)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum CopyContext {
    /// 单曲文案。
    Song,

    /// 歌单文案。
    Playlist,
}

/// action 的触发面(action_invocations.trigger)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum ActionTrigger {
    /// TUI 交互触发。
    Tui,

    /// CLI 子命令触发。
    Cli,
}

/// per-song KV 写操作类型(store_writes.op)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum StoreOp {
    /// 覆写值。
    Set,

    /// 自增计数。
    Inc,
}

/// 子进程的收束方式(spawns.outcome)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum SpawnOutcome {
    /// 正常退出。
    Exited,

    /// 被杀死。
    Killed,

    /// 起进程即失败。
    SpawnFailed,
}

/// 拒绝新连接的原因(connection_rejects.reason)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum RejectReason {
    /// 已有 client 占用。
    Busy,

    /// 协议版本不匹配。
    VersionMismatch,
}

/// 生命周期事件的主体(app_lifecycle.who)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum LifecycleWho {
    /// 常驻 daemon。
    Daemon,

    /// 连接的 client。
    Client,
}

/// 生命周期阶段(app_lifecycle.phase)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum LifecyclePhase {
    /// 启动。
    Start,

    /// 停止。
    Stop,
}

/// daemon 启动时的音频后端(app_lifecycle.audio_backend)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum AudioBackend {
    /// 拿到真实输出设备。
    Device,

    /// 降级空跑(无声卡 / headless)。
    Null,
}

/// 行为域事件:20 个变体一一对应行为域 20 张事件表。
///
/// 变体的具名字段 = 对应表的专有列(公共列 ts / session_id / actor 在
/// [`crate::StatsEvent::Behavior`] 层携带)。带 [`SongId`] 的字段落库时拆成
/// `ns` + `song_value` 两列;自由文本引用(歌单 ref / 缓存 key 等)用 `String`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BehaviorEvent {
    /// 一次搜索(searches)。
    Search {
        /// 搜索词原文;散列模式 / 隐私下为 `None`。
        query: Option<String>,

        /// 搜索词的稳定散列(保次数 / 去重)。
        query_hash: String,

        /// 搜索目标类型。
        kind: SearchTargetKind,

        /// 来源。
        source: SourceKind,

        /// 翻页页码。
        page: i64,

        /// 结果条数;失败 / 取消时未知为 `None`。
        result_count: Option<i64>,

        /// 结局。
        outcome: SearchOutcome,
    },

    /// 一次跳转(seeks)。
    Seek {
        /// 被跳转的歌曲。
        song: SongId,

        /// 起点 ms。
        from_ms: i64,

        /// 落点 ms。
        to_ms: i64,
    },

    /// 暂停 / 恢复(pauses)。
    Pause {
        /// 当前歌曲。
        song: SongId,

        /// 发生位置 ms。
        at_ms: i64,

        /// 暂停还是恢复。
        action: PauseAction,
    },

    /// 音量变化(volume_changes)。
    VolumeChange {
        /// 变前音量百分比。
        from_pct: i64,

        /// 变后音量百分比。
        to_pct: i64,
    },

    /// 播放模式切换(mode_changes)。
    ModeChange {
        /// 变前模式。
        from_mode: PlayMode,

        /// 变后模式。
        to_mode: PlayMode,
    },

    /// 收藏变更(love_changes)。
    LoveChange {
        /// 目标歌曲。
        song: SongId,

        /// 变更后是否收藏。
        loved: bool,

        /// 发起语义。
        origin: LoveOrigin,

        /// 远端镜像结果;不涉及远端为 `None`。
        remote_mirror: Option<RemoteMirror>,
    },

    /// 队列写操作(queue_ops)。
    QueueOp {
        /// 操作类型。
        op: QueueOp,

        /// 涉及的单曲;整队 / 清空等无单曲为 `None`。
        song: Option<SongId>,

        /// 影响条数。
        count: i64,
    },

    /// 歌单写操作(playlist_ops)。
    PlaylistOp {
        /// 操作类型。
        op: PlaylistOpKind,

        /// 歌单引用。
        playlist_ref: PlaylistRef,

        /// 涉及的单曲;整单级操作无单曲为 `None`。
        song: Option<SongId>,

        /// 影响歌曲数。
        song_count: i64,

        /// 结局。
        outcome: OpOutcome,

        /// 失败归类;成功时为 `None`。
        error_kind: Option<PlaylistError>,
    },

    /// 元数据 / 详情取数(fetches)。
    Fetch {
        /// 取数种类。
        fetch_kind: FetchKind,

        /// 来源。
        source: SourceKind,

        /// 取数目标引用;无为 `None`。
        target_ref: Option<String>,

        /// 触发方。
        trigger: FetchTrigger,

        /// 结局。
        outcome: FetchOutcome,

        /// 耗时 ms。
        latency_ms: i64,
    },

    /// 下载(downloads)。
    Download {
        /// 目标歌曲。
        song: SongId,

        /// 请求音质档串。
        quality: String,

        /// 落地格式;未知为 `None`。
        format: Option<String>,

        /// 结局。
        outcome: DownloadOutcome,

        /// 插件顶换结果。
        hooked: DownloadHook,

        /// 落地路径;未落地为 `None`。
        path: Option<String>,
    },

    /// 批量任务取消(task_cancels)。
    TaskCancel {
        /// 取消所用的过滤标签串。
        filter_tags: String,
    },

    /// 文案渲染(copy_renders)。
    CopyRender {
        /// 命中的模板序号。
        template_index: i64,

        /// 上下文类型。
        ctx_kind: CopyContext,

        /// 目标引用;无为 `None`。
        target_ref: Option<String>,

        /// 结局。
        outcome: OpOutcome,
    },

    /// action 调用(action_invocations)。
    ActionInvocation {
        /// action 名。
        name: String,

        /// 触发面。
        trigger: ActionTrigger,

        /// 结局。
        outcome: OpOutcome,
    },

    /// 脚本配置覆盖(config_overrides)。
    ConfigOverride {
        /// 被覆盖的配置路径。
        path: String,
    },

    /// per-song KV 写(store_writes)。
    StoreWrite {
        /// 归属歌曲。
        song: SongId,

        /// KV 键。
        key: String,

        /// 写操作类型。
        op: StoreOp,
    },

    /// 子进程派生(spawns)。
    Spawn {
        /// 程序名。
        program: String,

        /// 收束方式。
        outcome: SpawnOutcome,

        /// 退出码;非正常退出为 `None`。
        exit_code: Option<i64>,
    },

    /// 脚本事件总线消息(bus_messages)。
    BusMessage {
        /// 消息名。
        name: String,
    },

    /// 全屏切换(fullscreen_changes)。
    FullscreenChange {
        /// 切换后是否全屏。
        fullscreen: bool,
    },

    /// 拒绝连接(connection_rejects)。
    ConnectionReject {
        /// 拒绝原因。
        reason: RejectReason,
    },

    /// 进程生命周期(app_lifecycle)。
    AppLifecycle {
        /// 事件主体。
        who: LifecycleWho,

        /// 阶段。
        phase: LifecyclePhase,

        /// 音频后端;非 daemon 启动为 `None`。
        audio_backend: Option<AudioBackend>,

        /// 会话是否恢复;不适用为 `None`。
        session_restored: Option<bool>,

        /// client 版本串;非 client 为 `None`。
        client_version: Option<String>,
    },
}

impl BehaviorEvent {
    /// 事件对应的目标表名(= [`crate::StatsEvent::kind_name`] 的行为域分支)。
    pub(crate) fn table(&self) -> &'static str {
        match self {
            Self::Search { .. } => "searches",
            Self::Seek { .. } => "seeks",
            Self::Pause { .. } => "pauses",
            Self::VolumeChange { .. } => "volume_changes",
            Self::ModeChange { .. } => "mode_changes",
            Self::LoveChange { .. } => "love_changes",
            Self::QueueOp { .. } => "queue_ops",
            Self::PlaylistOp { .. } => "playlist_ops",
            Self::Fetch { .. } => "fetches",
            Self::Download { .. } => "downloads",
            Self::TaskCancel { .. } => "task_cancels",
            Self::CopyRender { .. } => "copy_renders",
            Self::ActionInvocation { .. } => "action_invocations",
            Self::ConfigOverride { .. } => "config_overrides",
            Self::StoreWrite { .. } => "store_writes",
            Self::Spawn { .. } => "spawns",
            Self::BusMessage { .. } => "bus_messages",
            Self::FullscreenChange { .. } => "fullscreen_changes",
            Self::ConnectionReject { .. } => "connection_rejects",
            Self::AppLifecycle { .. } => "app_lifecycle",
        }
    }

    /// 事件归属的来源 name(供 `exclude_sources` 在发送出口统一过滤)。
    ///
    /// 归属规则:显式 `source` 字段直取;携带 [`SongId`] 的取其 namespace;单曲缺席的
    /// 队列 / 歌单整体操作与全局事件(音量 / 模式 / 进程生命周期等)无来源,返回 `None`
    /// (不受排除影响)。`playlist_ref` / `target_ref` 一类自由文本引用不解析——它们不
    /// 保证是 qualified 串(synthetic 歌单名),猜测格式反而误伤。
    ///
    /// # Return:
    ///   来源 name;事件不归属任何来源为 `None`
    pub(crate) fn source_name(&self) -> Option<&str> {
        match self {
            Self::Search { source, .. } | Self::Fetch { source, .. } => Some(source.name()),
            Self::Seek { song, .. }
            | Self::Pause { song, .. }
            | Self::LoveChange { song, .. }
            | Self::Download { song, .. }
            | Self::StoreWrite { song, .. } => Some(song.namespace().name()),
            Self::QueueOp { song, .. } | Self::PlaylistOp { song, .. } => {
                song.as_ref().map(|s| s.namespace().name())
            }
            Self::VolumeChange { .. }
            | Self::ModeChange { .. }
            | Self::TaskCancel { .. }
            | Self::CopyRender { .. }
            | Self::ActionInvocation { .. }
            | Self::ConfigOverride { .. }
            | Self::Spawn { .. }
            | Self::BusMessage { .. }
            | Self::FullscreenChange { .. }
            | Self::ConnectionReject { .. }
            | Self::AppLifecycle { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::query_hash;

    /// query_hash:同词恒同散列、异词异散列、16 位十六进制。
    #[test]
    fn query_hash_is_stable_and_distinct() {
        assert_eq!(query_hash("周杰伦"), query_hash("周杰伦"), "同词恒同散列");
        assert_ne!(query_hash("周杰伦"), query_hash("林俊杰"), "异词异散列");
        let h = query_hash("test");
        assert_eq!(h.len(), 16, "16 位十六进制:{h}");
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()), "纯十六进制:{h}");
        // 固定向量:FNV-1a 手写算法不随版本漂移(空串 = offset basis)。
        assert_eq!(query_hash(""), "cbf29ce484222325");
    }
}
