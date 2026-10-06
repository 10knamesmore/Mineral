//! IPC 消息类型 — [`Request`] 与 [`Response`]。
//!
//! `Response` 的 variant 由调用方根据自己发出的 [`Request`] 决定预期;
//! server 业务失败统一返回 [`crate::OperationResult::Failed`]。

use mineral_model::{AlbumId, ArtistId, PlaylistId, Song, SongId};
use mineral_task::{Priority, TaskKind};
use serde::{Deserialize, Serialize};
use strum_macros::IntoStaticStr;
use thiserror::Error;

use crate::{DownloadId, DownloadTarget, PlayMode, QueueEditOutcome, QueueOp};

/// Atomic PlayQueue request 的 validation error。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum PlayQueueError {
    /// 提交了空 queue，无法选择 target occurrence。
    #[error("queue 不能为空")]
    Empty,

    /// Queue 超过 server 的 hard cap；请求整体拒绝，不 truncate。
    #[error("queue 长度 {len} 超过上限 {cap}")]
    CapacityExceeded {
        /// 请求提交的 Song 数。
        len: usize,

        /// Server 接受的最大 Song 数。
        cap: usize,
    },

    /// Target 不是本次提交 Song Vec 中的有效 queue index。
    #[error("target {target} 越界，queue 长度为 {len}")]
    TargetOutOfBounds {
        /// 请求提交的 0-based target index。
        target: usize,

        /// 请求提交的 Song 数。
        len: usize,
    },

    /// IPC transport 或响应 contract 不可用，queue 未确认起播。
    #[error("队列起播未确认: {message}")]
    Unavailable {
        /// 仅用于日志诊断；前端据 `Unavailable` 类别生成提示。
        message: String,
    },
}

/// 队列语境的 wire 形态:client 告知一个队列「来自哪」,server 映射进埋点 `QueueContext`
/// 后随该队列每个 plays 行继承(单一 origin 有归属漏洞:从歌单点第一首后连播 20 首,
/// 后 19 行只知 AutoAdvance,「最常听的歌单」就断了)。id 用 mineral_model 类型,天然可序列化。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueContextWire {
    /// 搜索结果(携搜索词)。
    Search {
        /// 触发该队列的搜索词。
        query: String,
    },

    /// 歌单 tracks(含聚合收藏这类 synthetic 歌单)。
    Playlist {
        /// 歌单 ID。
        id: PlaylistId,

        /// 歌单显示名快照(队列建立时刻);拿不到给 `None`。
        #[serde(default)]
        name: Option<String>,
    },

    /// 专辑详情。
    Album {
        /// 专辑 ID。
        id: AlbumId,

        /// 专辑页标题快照(队列建立时刻);拿不到给 `None`。
        #[serde(default)]
        name: Option<String>,
    },

    /// 艺人详情。
    Artist {
        /// 艺人 ID。
        id: ArtistId,

        /// 艺人页标题快照(队列建立时刻);拿不到给 `None`。
        #[serde(default)]
        name: Option<String>,
    },

    /// 手动攒的队列(insert_next / append 散曲)。
    Manual,

    /// 未标注(缺省)。
    Unknown,
}

/// 一首歌的播放统计快照(IPC 出参)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SongStatsWire {
    /// 在 Mineral 中自然播完的次数(`finish_reason = eof`)。
    pub play_count: u32,

    /// 跳过次数。
    pub skip_count: u32,

    /// 累计收听毫秒。
    pub total_listen_ms: u64,

    /// 最近播放 unix ms(无则 None)。
    pub last_played_at: Option<i64>,

    /// 是否 loved。
    pub loved: bool,
}

/// client → daemon 的业务请求。
///
/// 转成静态字符串时返回 snake_case 请求名称，供日志标识操作。
#[derive(Clone, Debug, Serialize, Deserialize, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum Request {
    /// 保存或管理用户歌单。
    Playlist {
        /// 要执行的歌单操作。
        op: crate::PlaylistOp,
    },

    // ---- 播放控制 ----
    /// 暂停。
    Pause,

    /// 从暂停恢复。
    Resume,

    /// 停止当前曲目。
    Stop,

    /// 跳到绝对位置(ms),latest-wins。
    Seek(u64),

    /// 设置音量百分比(0..=100)。
    SetVolume(u8),

    /// Enumerates CPAL output devices on the daemon host.
    AudioOutputs,

    /// Selects the output route for this daemon session.
    SetAudioOutput(mineral_audio::OutputTarget),

    // ---- 任务调度 ----
    /// 提交一个任务。返回 [`Response::Ok`]，不等待任务完成。
    SubmitTask(TaskKind, Priority),

    // ---- Player 业务 ----(server 持权威 PlayerState)
    /// Selects one song as current playback. Returns [`Response::Ok`].
    /// `Box` 是为了避免 enum 体积膨胀(`Song` 比平均 variant 大很多)。
    PlaySong(Box<Song>),

    /// 原子替换 queue 并起播 request-local target occurrence。
    ///
    /// Server 先完整验证空 queue、hard cap 与 target；失败返回
    /// [`Response::PlayQueue`] 的 structured error 且状态完全不变。Shuffle 以 target
    /// occurrence 为起播项，不按 SongId first-match。
    PlayQueue {
        /// 本次提交的新 queue。
        songs: Vec<Song>,

        /// `songs` 内的 0-based queue index，不是 CollectionIndex。
        target: usize,

        /// 队列语境(埋点 provenance:该队列来自搜索 / 歌单 / 专辑 / 艺人 / 手动)。
        context: QueueContextWire,
    },

    /// 按输入顺序将整组歌曲插到当前曲之后，不动当前曲与队列级来源语境。
    /// 空队列从头插入；Shuffle 模式同步插入 original_queue。
    /// 空批次或整组超过队列容量时不修改队列。返回 [`Response::Ok`]。
    QueueInsertNext {
        /// 待插播的整组歌曲，保留顺序和重复项。
        songs: Vec<Song>,

        /// 这组歌曲的来源语境，起播时覆盖队列级语境。
        context: QueueContextWire,
    },

    /// 按输入顺序将整组歌曲追加到队尾，不动当前曲与队列级来源语境。
    /// Shuffle 模式同步追加 original_queue。
    /// 空批次或整组超过队列容量时不修改队列。返回 [`Response::Ok`]。
    QueueAppend {
        /// 待追加的整组歌曲，保留顺序和重复项。
        songs: Vec<Song>,

        /// 这组歌曲的来源语境，起播时覆盖队列级语境。
        context: QueueContextWire,
    },

    /// 队列结构编辑:删除 / 重排 / 批量清理 / 脚本变换 / 撤销。
    ///
    /// 单变体承载全部编辑而非按操作平铺,是因为埋点入口层对每个 `Request` 变体都要补一条
    /// 归属;op 判别落成埋点表的一列后,粒度并不损失。
    /// 返回 [`Response::QueueEdited`]。
    QueueEdit {
        /// 具体操作。
        op: QueueOp,
    },

    /// 拉全部已注册 channel 的能力表(启动握手时一次,断连重连后再拉)。
    /// 返回 [`Response::ChannelCaps`]。
    ChannelCaps,

    /// 查询 daemon 当前提供的队列变换与播放统计能力。返回 [`Response::ServiceInfo`]。
    ServiceInfo,

    /// `m` 键循环 PlayMode。返回 [`Response::Ok`]。
    CyclePlayMode,

    /// 直接设置播放模式(系统媒体控件、脚本与 CLI 的同入口)。设成当前同档为 no-op:
    /// 不重洗队列、不记 `mode_changes`。返回 [`Response::Ok`]。
    SetPlayMode(PlayMode),

    /// `p` 键:进度 > 阈值时回开头,否则跳上一首。返回 [`Response::Ok`]。
    PrevOrRestart,

    /// `n` 键:按当前 mode 切下一首。返回 [`Response::Ok`]。
    NextSong,

    // ---- 诊断 ----
    /// 拉一次 daemon 进程信息(pid 等)。返回 [`Response::DaemonInfo`]。
    /// 给运维 / 性能剖析定位 daemon 进程用(`mineral status` 会打出 pid)。
    DaemonInfo,

    // ---- love / 统计 ----
    /// 切换一首歌的喜欢(♥)状态。返回 [`Response::LoveToggled`]，携带切换后的新状态。
    ///
    /// 携带整首 [`Song`] 而非裸 id:server 落 love 的同时把元数据写进 persist,
    /// 跨源聚合视图(全源收藏)才能离线重建出歌名 / 艺人 / 时长。
    ToggleLove(Box<Song>),

    /// 查询一首歌的 Mineral 本地播放统计。返回 [`Response::SongStats`]；当前未采集该
    /// source 时为 `None`，启用采集但从未播放时为零值。
    QuerySongStats(SongId),

    // ---- 下载 ----
    /// 提交单曲或歌单下载。返回 [`Response::Ok`]；下载状态由汇总和列表查询提供。
    Download(DownloadTarget),

    /// Stop 一个 Song download。已知 identity 返回 [`Response::Ok`]，未知 identity 返回结构化失败。
    StopDownload(DownloadId),

    // ---- per-song 持久 KV ----
    /// 读 per-song 持久值(开放 key)。返回 [`Response::StoreValue`](未命中 `Nil`)。
    StoreGet {
        /// 目标歌。
        song: SongId,

        /// 开放键(如 `plugin.skipcount`)。
        key: String,
    },

    /// 写 per-song 持久值(开放 key;`Nil` 删除)。返回 [`Response::Ok`]。
    StoreSet {
        /// 目标歌。
        song: SongId,

        /// 开放键。
        key: String,

        /// 标量值。
        value: crate::StoreValue,
    },

    // ---- UI 状态上报 ----
    /// client 上报终端 UI 状态(resize / 全屏切换时发)。daemon 按连接归属记录,
    /// 灌属性树 `terminal` 复合属性——多终端平等,属性取最近上报的那条,
    /// 断开只清自己的。返回 [`Response::Ok`]。
    TerminalState {
        /// 终端行数。
        rows: u16,

        /// 终端列数。
        cols: u16,

        /// 是否处于全屏播放态。
        fullscreen: bool,

        /// 终端窗口是否持有输入焦点(终端经 focus 事件上报;不支持
        /// mode 1004 的终端收不到事件,client 恒报 `true`)。
        focused: bool,
    },

    // ---- 生命周期 ----
    /// 请求 daemon 优雅退出:先回 [`Response::Ok`] ack,随后走与 SIGTERM
    /// 完全相同的收尾(停 server、清 socket)。`mineral stop` 与 TUI 的
    /// 「退出并停止 daemon」都走这条;对 attach 模式它是唯一通路
    /// (client 没有 daemon 的 pid,发不了信号)。
    ///
    /// **任一** client 都可发起,停机殃及所有已连接 client——单人自用语义,
    /// 不设权限仲裁。
    Shutdown,
}

/// Server → Client 应答。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Response {
    /// 保存成功后的独立歌单身份。
    PlaylistSaved {
        /// 新创建的 Mineral 歌单身份。
        id: PlaylistId,
    },

    /// CPAL output devices returned by [`Request::AudioOutputs`].
    AudioOutputs(Vec<mineral_audio::OutputDevice>),

    /// 无返回值的命令成功。
    Ok,

    /// 对应 [`Request::DaemonInfo`]。
    DaemonInfo {
        /// daemon 进程 pid(`std::process::id()`)。
        pid: u32,
    },

    /// 对应 [`Request::ToggleLove`]:切换后的新 loved 状态。
    LoveToggled(bool),

    /// 对应 [`Request::QueueEdit`]:本次编辑的结果。
    QueueEdited(QueueEditOutcome),

    /// 对应 [`Request::PlayQueue`]：成功或 structured validation error。
    PlayQueue(Result<(), PlayQueueError>),

    /// 对应 [`Request::QuerySongStats`]：当前采集可用时返回统计，否则为 `None`。
    SongStats(Option<SongStatsWire>),

    /// 对应 [`Request::ChannelCaps`]:每个已注册 channel 的能力声明。
    ChannelCaps(Vec<(mineral_model::SourceKind, mineral_channel_core::ChannelCaps)>),

    /// 对应 [`Request::ServiceInfo`]：daemon 当前可用的服务能力。
    ServiceInfo(crate::ServiceInfo),

    /// 对应 [`Request::StoreGet`]:标量值(未命中 `Nil`)。
    StoreValue(crate::StoreValue),

    /// 服务端处理失败；诊断详情不用于 UI 展示。
    Error(crate::OperationFailure),
}
