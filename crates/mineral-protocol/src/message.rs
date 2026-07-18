//! IPC 消息类型 — [`Request`] 与 [`Response`]。
//!
//! 与 [`mineral_server::ClientHandle`] 的方法 1:1 对应;`Response` 的 variant 由
//! 调用方根据自己发的 `Request` 决定预期。错误统一走 [`Response::Error`]。

use mineral_audio::AudioSnapshot;
use mineral_model::{AlbumId, ArtistId, MediaUrl, PlaylistId, Song, SongId};
use mineral_task::{Priority, Snapshot, TaskId, TaskKind};
use serde::{Deserialize, Serialize};

use crate::{CancelFilter, PlayerSync, PlayerVersions};

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

/// 下载进度快照(client 每 tick 轮询,驱动 top-center 进度弹窗)。
///
/// `active == false` 时其余字段无意义(无下载在跑),client 据此收起弹窗。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadProgress {
    /// 当前是否有下载任务在跑。
    pub active: bool,

    /// 本批已完成首数。
    pub done: usize,

    /// 本批总首数(单曲为 1)。
    pub total: usize,

    /// 当前这首已下字节。
    pub bytes_done: u64,

    /// 当前这首总字节(拿不到为 0,进度条退化为 spinner 语义)。
    pub bytes_total: u64,

    /// 当前瞬时下载速度(字节/秒,已平滑)。
    pub speed_bps: u64,

    /// 队列中**还在等待**的批数(不含正在下的这批)。串行下载,>0 表示后面还排着。
    pub queued: usize,

    /// 已完成批次计数(每批下完 +1);client 据其增长触发一次「完成提示」。
    pub result_seq: u64,

    /// 最近完成那批**真正下载**的成功首数(配合 `result_seq` 读)。
    pub last_ok: usize,

    /// 最近完成那批**已存在跳过**的首数(配合 `result_seq` 读)。
    pub last_skip: usize,

    /// 最近完成那批的失败首数(配合 `result_seq` 读)。
    pub last_fail: usize,
}

/// 下载目标:单曲(tracks 视图选中)或整张歌单(playlist 视图选中)。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DownloadTarget {
    /// 下载一首歌。`Box` 避免 enum 体积膨胀(`Song` 较大)。
    Song(Box<Song>),

    /// 下载整张歌单的全部曲目(server 端自行拉 tracks)。
    Playlist(PlaylistId),
}

/// 一首歌的播放统计快照(IPC 出参)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SongStatsWire {
    /// 完整播放次数。
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

/// Client → Server 命令。每条 [`Request`] 一定有一条对应的 [`Response`]。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    // ---- 播放控制 ----
    /// 切到指定 URL 播放(对应 [`mineral_server::ClientHandle::play`])。
    Play(MediaUrl),

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

    /// 拉一次音频快照。返回 [`Response::AudioSnapshot`]。
    AudioSnapshot,

    // ---- 任务调度 ----
    /// 提交一个任务。返回 [`Response::TaskId`]。
    SubmitTask(TaskKind, Priority),

    /// 按过滤条件批量取消任务。返回 [`Response::Ok`]。
    CancelTasks(CancelFilter),

    /// 拉一次 scheduler 状态快照。返回 [`Response::TaskSnapshot`]。
    TaskSnapshot,

    // ---- Player 业务 ----(server 持权威 PlayerState)
    /// Client 选了一首歌。Server 内部跑完整 play 流程(cancel 旧 SongUrl/Lyrics、
    /// audio.stop、记录 current_song、命中 prefetched 直接 audio.play、否则
    /// 提交新 SongUrl/Lyrics 任务,等 PlayUrlReady 后内部 audio.play)。
    /// 返回 [`Response::Ok`]。
    /// `Box` 是为了避免 enum 体积膨胀(`Song` 比平均 variant 大很多)。
    PlaySong(Box<Song>),

    /// 替换 queue + 设当前位置。Shuffle 模式下 server 端洗牌。
    /// 返回 [`Response::Ok`]。
    SetQueue {
        /// 新 queue。
        queue: Vec<Song>,
        /// queue 中作为「当前」的歌 id;server 据此设 queue_sel。
        target_id: mineral_model::SongId,
        /// 队列语境(埋点 provenance:该队列来自搜索 / 歌单 / 专辑 / 艺人 / 手动)。
        context: QueueContextWire,
    },

    /// 插播:插到当前曲之后,不动播放上下文与当前曲。
    /// Shuffle 模式下同步插入 original_queue(当前曲后)。返回 [`Response::Ok`]。
    QueueInsertNext {
        /// 待插播的歌(`Box` 避免 enum 体积膨胀)。
        song: Box<Song>,
        /// 该曲的来源语境(埋点 per-song 覆盖:插队散曲不继承队列级 context)。
        context: QueueContextWire,
    },

    /// 追加到队列末尾,不动播放上下文与当前曲。
    /// Shuffle 模式下同步追加 original_queue 末尾。返回 [`Response::Ok`]。
    QueueAppend {
        /// 待追加的歌(`Box` 避免 enum 体积膨胀)。
        song: Box<Song>,
        /// 该曲的来源语境(埋点 per-song 覆盖:同插播)。
        context: QueueContextWire,
    },

    /// 拉全部已注册 channel 的能力表(启动握手时一次,断连重连后再拉)。
    /// 返回 [`Response::ChannelCaps`]。
    ChannelCaps,

    /// `m` 键循环 PlayMode。返回 [`Response::Ok`]。
    CyclePlayMode,

    /// `p` 键:进度 > 阈值时回开头,否则跳上一首。返回 [`Response::Ok`]。
    PrevOrRestart,

    /// `n` 键:按当前 mode 切下一首。返回 [`Response::Ok`]。
    NextSong,

    /// 版本门控的播放状态同步:client 报自己已有的版本号(0 = 一无所有),
    /// server 仅在版本落后时附带对应重段。启动与每 tick 同一条路径。
    /// 返回 [`Response::PlayerSync`]。
    PlayerSync(PlayerVersions),

    // ---- PCM 流 ----
    /// 拉最多 N 个 f32 PCM 样本(单声道,FFT 输入用)。
    /// 返回 [`Response::PcmData`]。
    PullPcm(usize),

    // ---- 诊断 ----
    /// 拉一次 daemon 进程信息(pid 等)。返回 [`Response::DaemonInfo`]。
    /// 给运维 / 性能剖析定位 daemon 进程用(`mineral status` 会打出 pid)。
    DaemonInfo,

    // ---- love / 统计 ----
    /// 切换一首歌的喜欢(♥)状态。返回 [`Response::LoveToggled`](切换后的新状态)。
    ///
    /// 携带整首 [`Song`] 而非裸 id:server 落 love 的同时把元数据写进 persist,
    /// 跨源聚合视图(全源收藏)才能离线重建出歌名 / 艺人 / 时长。
    ToggleLove(Box<Song>),

    /// 查询一首歌的播放统计。返回 [`Response::SongStats`]。
    QuerySongStats(SongId),

    // ---- 下载 ----
    /// 下载(永久导出 + 顺带填 cache)单曲 / 整张歌单。fire-and-forget,server 后台跑。
    /// 返回 [`Response::Ok`]。
    Download(DownloadTarget),

    /// 拉一次下载进度快照(TUI 进度弹窗 / CLI status 用)。返回 [`Response::DownloadProgress`]。
    DownloadProgress,

    // ---- 脚本 ----
    /// 触发脚本具名动作(`mineral.action` 注册)。成功返回 [`Response::Ok`];
    /// 未注册 / 脚本未启用 / 回调失败返回 [`Response::Error`]。
    InvokeAction {
        /// 动作注册名(config.lua 里 `mineral.action` 的第一个参数)。
        name: String,

        /// 按键瞬间的 client 上下文(TUI 采集;CLI 等无界面触发面为 `None`)。
        ctx: Option<crate::KeyContext>,

        /// 调用位置实参(CLI `mineral action <name> <args...>` 采集;
        /// TUI 键位触发为空 `Vec`)。Lua 回调经 `ctx.args` 读取(恒为数组)。
        args: Vec<String>,
    },

    /// 渲染一个用户复制模板(config.lua `copy.templates[index]` 的回调,daemon
    /// 脚本运行时执行):函数收 `ctx` 投影成的 Lua 表,返回要进剪贴板的文本。
    /// 返回 [`Response::CopyText`];无脚本运行时 / 下标越界 / 回调失败走其 `Err` 侧。
    RenderCopyTemplate {
        /// 模板在 `copy.templates` 数组中的下标(client 与 daemon eval 同一份
        /// config,序号天然对位)。
        index: usize,

        /// 模板作用的实体(client 侧光标所指,数据随请求带过去,daemon 无需
        /// 反查任何视图状态)。
        ctx: CopyTemplateCtx,
    },

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

    /// per-song 数值自增。返回 [`Response::StoreValue`](自增后的值)。
    StoreInc {
        /// 目标歌。
        song: SongId,
        /// 开放键。
        key: String,
        /// 增量(可负)。
        delta: i64,
    },

    /// 拉取脚本 `mineral.bind` 产生的键绑定表(client 启动 / 配置重载后调,
    /// 合进自己的 keymap)。返回 [`Response::ScriptBinds`](无脚本为空)。
    ScriptBinds,

    // ---- UI 状态上报 ----
    /// client 上报终端 UI 状态(resize / 全屏切换时发)。daemon 按连接归属记录,
    /// 灌属性树 `terminal` 复合属性供脚本 observe——多终端平等,属性取最近
    /// 上报的那条,断开只清自己的。返回 [`Response::Ok`]。
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
#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    /// 无返回值的命令成功(play / pause / resume / stop / seek / set_volume / cancel_tasks)。
    Ok,

    /// 对应 [`Request::AudioSnapshot`]。
    AudioSnapshot(AudioSnapshot),

    /// 对应 [`Request::SubmitTask`]。
    TaskId(TaskId),

    /// 对应 [`Request::TaskSnapshot`]。
    TaskSnapshot(Snapshot),

    /// 对应 [`Request::PlayerSync`]。`Box` 避免 enum 体积膨胀。
    PlayerSync(Box<PlayerSync>),

    /// 对应 [`Request::PullPcm`]。
    PcmData {
        /// 0..=N 个样本(可能短于 caller 请求的 N;0 = 当前没数据)。
        samples: Vec<f32>,
        /// 当前 audio 采样率(Hz);0 = 还没在播。client 用它驱动 fft。
        sample_rate: u32,
    },

    /// 对应 [`Request::DaemonInfo`]。
    DaemonInfo {
        /// daemon 进程 pid(`std::process::id()`)。
        pid: u32,
    },

    /// 对应 [`Request::ToggleLove`]:切换后的新 loved 状态。
    LoveToggled(bool),

    /// 对应 [`Request::QuerySongStats`]:命中返回统计,无记录返回 None。
    SongStats(Option<SongStatsWire>),

    /// 对应 [`Request::DownloadProgress`]:当前下载进度快照。
    DownloadProgress(DownloadProgress),

    /// 对应 [`Request::ChannelCaps`]:每个已注册 channel 的能力声明。
    ChannelCaps(Vec<(mineral_model::SourceKind, mineral_channel_core::ChannelCaps)>),

    /// 对应 [`Request::StoreGet`] / [`Request::StoreInc`]:标量值(未命中 `Nil`)。
    StoreValue(crate::StoreValue),

    /// 对应 [`Request::ScriptBinds`]:脚本 bind 表(注册顺序;无脚本为空)。
    ScriptBinds(Vec<crate::ScriptBind>),

    /// 对应 [`Request::RenderCopyTemplate`]:`Ok` = 回调返回的剪贴板文本,
    /// `Err` = 人读错误短文(无脚本运行时 / 下标越界 / 回调失败 / 超时被中断)。
    CopyText(Result<String, String>),

    /// 服务端处理失败 / 当前不接受新 client / 协议异常。文本人读即可。
    Error(String),
}

/// 复制模板回调作用的实体:client 侧光标所指,整体随请求传输
/// (含已加载曲目等,daemon 端零状态反查)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CopyTemplateCtx {
    /// 一首歌(`context = "song"` 的模板)。
    Song(Box<mineral_model::Song>),

    /// 一张歌单,`songs` 为 client 已加载的曲目(`context = "playlist"` 的模板)。
    Playlist(Box<mineral_model::Playlist>),

    /// 一张专辑(`context = "album"` 的模板)。
    Album(Box<mineral_model::Album>),

    /// 一个 artist(`context = "artist"` 的模板)。
    Artist(Box<mineral_model::Artist>),
}
