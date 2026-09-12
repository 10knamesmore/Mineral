//! 与传输无关的逻辑会话协议:请求批、结果、订阅更新、分片与控制消息。
//!
//! 会话不是 socket、也不是一次 HTTP 请求 —— [`SessionMessage`] 是 client 与 daemon
//! 交换的结构化消息,[`crate::MessageBatch`] 是一次承载(一批消息)。编码、framing
//! 与传输错误留给 wire adapter;本模块只定义逻辑形状。
//!
//! 关键语义:
//! - **请求配对**:[`SessionRequest::id`] 由 client 分配,daemon 在 [`SessionResult`]
//!   原样回带;同方向按会话次序交付,应答可乱序。
//! - **订阅**:[`SubscriptionId`] 在 session 内有效,取消后旧 id 的更新必须被忽略。
//!   每次更新带单调 `version`;分片组共享同一 `(subscription, version)`,收齐后原子应用。
//! - **结果分类**:[`OperationResult`] 区分已应用、已受理、查询载荷与业务失败;
//!   「未提交」与「发送后结果未知」由 client 侧表达,不伪造为业务默认值。

use mineral_audio::AudioSnapshot;
use mineral_model::Song;
use mineral_task::Snapshot;
use serde::{Deserialize, Serialize};

use crate::{
    ClientInfo, DownloadId, DownloadStatus, DownloadSummary, Event, PlayerSync, PlayerVersions,
    Request, Response, ServerHello, SongDownloadView, Subscription,
};

/// 会话内订阅标识。由 client 单调分配;取消后重订必须换新 id。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SubscriptionId(u64);

impl SubscriptionId {
    /// 用裸值构造(client 侧分配入口)。
    ///
    /// # Params:
    ///   - `value`: 会话内唯一值
    #[must_use]
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// 裸值(日志 / 调试)。
    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

/// 会话内请求标识。由 client 单调分配,daemon 在 [`SessionResult`] 原样回带。
///
/// 只在单条会话内有意义(断链即退出,无跨会话陈旧 id 问题)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RequestId(u64);

impl RequestId {
    /// 用裸值构造(client 侧自增计数器单入口)。
    ///
    /// # Params:
    ///   - `value`: 会话内唯一值
    #[must_use]
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// 裸值(日志 / 调试)。
    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

/// 一次订阅声明的主题。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubscriptionTopic {
    /// 播放状态(队列 / 当前曲 / 歌词 / 媒体事实),按 [`PlayerSync`] 版本门控。
    Player,

    /// 播放锚点([`AudioSnapshot`]):client 用本地单调钟推进展示位置。
    Playback,

    /// scheduler 任务摘要([`Snapshot`])。
    Tasks,

    /// 下载摘要([`DownloadSummary`]):小、常驻。
    DownloadsSummary,

    /// 下载明细:静态行 + 进度增量,按需订阅。
    DownloadsDetail,

    /// PCM 样本流:有界推送,带播放代次与缺口标记。
    Pcm,

    /// 事件类别(Toast / Lifecycle / Config / WindowTitle / Task / Bus / Property)。
    Events(Subscription),
}

/// 订阅请求:client 声明 id、主题与已持播放版本(Player 主题用于裁剪重段)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubscribeRequest {
    /// 本次订阅的会话内标识。
    pub id: SubscriptionId,

    /// 订阅主题。
    pub topic: SubscriptionTopic,

    /// Player 主题:client 已持有的播放版本(`None` = 一无所有,收全量)。
    pub known_player: Option<PlayerVersions>,
}

/// client → daemon 的一条请求(带会话内配对 id)。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionRequest {
    /// 配对标识(client 分配)。
    pub id: RequestId,

    /// 业务请求体。
    pub request: Request,
}

/// daemon → client 的一条请求结果。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionResult {
    /// 与 [`SessionRequest::id`] 相同的配对标识。
    pub id: RequestId,

    /// 本次请求的结论。
    pub result: OperationResult,
}

/// 一条请求的结论。
///
/// 「未提交」与「已发送但结果未知」不在本枚举内:前者是 client 侧入队失败,后者是
/// 连接断开后的收束状态,都不属于 daemon 的结论。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum OperationResult {
    /// 操作已在 daemon 应用;订阅更新与本结果独立交付,到达顺序不作保证。
    Applied,

    /// 后台工作已受理(完成由领域状态表达,不另发完成应答)。
    Accepted,

    /// 查询载荷(与请求一一对应,只回发起者)。
    Query(Box<Response>),

    /// 业务失败(结构化,前端据 [`FailureKind`] 生成展示文案)。
    Failed(OperationFailure),
}

/// 业务失败的类别与可诊断细节。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationFailure {
    /// 失败类别(前端文案路由)。
    pub kind: FailureKind,

    /// 人读诊断细节(日志 / 兜底展示;不作为稳定契约)。
    pub detail: String,
}

/// 失败类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureKind {
    /// 请求参数不合法(校验失败,状态未变)。
    Invalid,

    /// 目标不存在。
    NotFound,

    /// 依赖能力当前不可用(脚本未启用 / 未登录 / 下载不可用等)。
    Unavailable,

    /// 与当前状态冲突(版本落后、重复操作等)。
    Conflict,

    /// daemon 内部错误(已记完整错误链)。
    Internal,
}

/// 版本化订阅更新:同一 `(subscription, version)` 的 `parts` 条消息按 `index` 组装。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateEnvelope {
    /// 目标订阅。
    pub subscription: SubscriptionId,

    /// 该订阅的单调版本(每次逻辑更新 +1)。
    pub version: u64,

    /// 本逻辑更新的总消息数(含自身);`1` 表示无需组装。
    pub parts: u32,

    /// 本消息在组内的 0-based 序号。
    pub index: u32,

    /// 本消息承载的载荷片段。
    pub payload: UpdatePayload,
}

/// 订阅更新载荷。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum UpdatePayload {
    /// 播放状态头段(轻段 + 可选 current 重段);大队列经 [`Self::PlayerQueuePart`] 分片。
    Player {
        /// 版本门控结果(`queue` 字段在分片时为空,由 queue part 组装)。
        sync: Box<PlayerSync>,

        /// 本组 queue 分片数(0 = 队列内联在 `sync.queue` 或本次无 queue 变更)。
        queue_parts: u32,
    },

    /// 播放队列分片(与头段同一版本原子组装)。
    PlayerQueuePart {
        /// 本片在整段队列中的 0-based 起始下标。
        offset: u32,

        /// 本片携带的队列条目。
        queue: Vec<Song>,

        /// shuffle 原序(仅第一片携带,其余片为 `None`)。
        original_queue: Option<Vec<Song>>,
    },

    /// 播放锚点(位置 / 音量 / 状态;client 用本地单调钟推进展示)。
    Playback(Box<AudioSnapshot>),

    /// scheduler 任务摘要。
    Tasks(Box<Snapshot>),

    /// 下载摘要。
    DownloadsSummary(DownloadSummary),

    /// 下载明细完整快照(行数不超阈值时内联)。
    DownloadsDetailSnapshot(Vec<SongDownloadView>),

    /// 下载明细快照头(行经 [`Self::DownloadsDetailPart`] 分片)。
    DownloadsDetailHead {
        /// 快照总行数(与分片 offset 对齐校验)。
        total: u32,
    },

    /// 下载明细快照分片。
    DownloadsDetailPart {
        /// 本片在整表中的 0-based 起始下标。
        offset: u32,

        /// 本片携带的明细行。
        rows: Vec<SongDownloadView>,
    },

    /// 下载明细增量(静态字段只在快照 / Upsert 出现;进度增量不带整首 Song)。
    DownloadsDetailDelta(DownloadDetailDelta),

    /// PCM 样本块。
    Pcm(PcmChunk),

    /// 事件类别推送(按订阅集过滤)。
    Event(Box<Event>),
}

/// 下载明细增量包:顺序变化 + 行变更,同一版本原子应用。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DownloadDetailDelta {
    /// 新的完整展示顺序(active / queued / 最近终态);`None` = 顺序未变。
    pub order: Option<Vec<DownloadId>>,

    /// 行变更(按顺序应用)。
    pub changes: Vec<DownloadDetailUpdate>,
}

/// 下载明细增量。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DownloadDetailUpdate {
    /// 新增或静态字段变化(整行替换)。
    Upsert(Box<SongDownloadView>),

    /// 轻量进度变化:只带会连续变化的字段。
    Progress {
        /// 目标行。
        id: DownloadId,

        /// 当前生命周期状态。
        status: DownloadStatus,

        /// 已写字节。
        bytes_done: u64,

        /// 声明总字节(未知为 `None`)。
        bytes_total: Option<u64>,

        /// 平滑速度(字节/秒)。
        speed_bps: u64,

        /// 失败链(仅失败态有值)。
        failure: Option<String>,
    },

    /// 移除(历史裁剪 / 收束)。
    Remove(DownloadId),
}

/// PCM 样本块。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PcmChunk {
    /// 播放代次:起播 / seek / 切输入格式 / 上游断开时 +1;client 据此重置 FFT 窗口。
    pub generation: u64,

    /// 本块首个样本的绝对流位置(样本数,单调)。
    pub position: u64,

    /// 与上一块之间是否存在缺口(上游丢样本 / 订阅者落后跳窗)。
    pub gap: bool,

    /// 采样率(Hz);`None` = 尚未就绪(不用 0 充当未知)。
    pub sample_rate: Option<u32>,

    /// 单声道样本。
    pub samples: Vec<f32>,
}

/// 会话关闭原因(双向使用)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CloseReason {
    /// client 主动结束会话(进程退出 / 命令完成)。
    ClientClosed,

    /// daemon 正在退出。
    ServerShutdown,

    /// 协议违规(首帧不是 Hello、版本不匹配等)。
    Protocol {
        /// 人读细节。
        detail: String,
    },

    /// 背压或组装超限。
    Backpressure {
        /// 人读细节。
        detail: String,
    },

    /// 传输层断开。
    Transport {
        /// 人读细节。
        detail: String,
    },
}

/// 一次承载的会话消息批次:同方向按批次内顺序交付。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageBatch {
    /// 本批消息(可为空批,仅用于 flush / 心跳)。
    pub messages: Vec<SessionMessage>,
}

impl MessageBatch {
    /// 空批。
    #[must_use]
    pub fn empty() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    /// 单条消息一批。
    ///
    /// # Params:
    ///   - `message`: 消息
    #[must_use]
    pub fn one(message: SessionMessage) -> Self {
        Self {
            messages: vec![message],
        }
    }

    /// 本批消息数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// 是否为空批。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// 会话消息。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SessionMessage {
    // ---- client → daemon ----
    /// 首帧:client 身份(版本 + 自报名)。
    Hello(ClientInfo),

    /// 一批请求(会话层自动合批的产物)。
    Requests(Vec<SessionRequest>),

    /// 建立订阅。
    Subscribe(SubscribeRequest),

    /// 取消订阅(旧 id 的后续更新由 client 忽略)。
    Unsubscribe(SubscriptionId),

    /// 客户端发现版本缺口 / 组装失败,请求重新快照。
    Resync(SubscriptionId),

    /// client 主动关闭会话。
    Close(CloseReason),

    // ---- daemon → client ----
    /// 握手应答(先于任何结果 / 更新)。
    Welcome(ServerHello),

    /// 一批请求结果(可与更新交错)。
    Results(Vec<SessionResult>),

    /// 版本化订阅更新。
    Update(UpdateEnvelope),

    /// daemon 主动关闭会话(拒绝、停机、背压收束)。
    Goodbye(CloseReason),
}

/// 组装分片时的硬上限(超过即明确失败,不做无限缓冲)。
pub mod assembly_limits {
    /// 单个逻辑更新的最大分片数。
    pub const MAX_PARTS: u32 = 256;

    /// 单会话同时进行中的最大组装组数。
    pub const MAX_GROUPS: usize = 4;

    /// 单个逻辑更新组装后的最大载荷字节估算。
    pub const MAX_GROUP_BYTES: usize = 8 * 1024 * 1024;
}

/// 播放队列分片阈值:超过该行数时 queue 重段按片下发,让控制帧可穿插。
pub const PLAYER_QUEUE_PART_ROWS: usize = 256;

/// 下载明细快照分片阈值。
pub const DOWNLOAD_DETAIL_PART_ROWS: usize = 512;

/// 把一条播放更新拆成 1..N 条 [`SessionMessage::Update`](同 subscription/version)。
///
/// 小队列整段内联(`parts == 1`);大队列拆成头段(无 queue)+ 若干
/// [`UpdatePayload::PlayerQueuePart`],让其他订阅 / 结果帧在片间穿插。
///
/// # Params:
///   - `subscription`: 目标订阅
///   - `version`: 该订阅的本次逻辑版本
///   - `sync`: 版本门控结果(含 queue 重段时才会分片)
///
/// # Return:
///   按发送顺序排列的消息。
pub fn fragment_player_update(
    subscription: SubscriptionId,
    version: u64,
    mut sync: PlayerSync,
) -> Vec<SessionMessage> {
    let queue = sync.queue.take();
    let Some(mut queue) = queue else {
        return vec![SessionMessage::Update(UpdateEnvelope {
            subscription,
            version,
            parts: 1,
            index: 0,
            payload: UpdatePayload::Player {
                sync: Box::new(sync),
                queue_parts: 0,
            },
        })];
    };
    let rows = queue.queue.len();
    if rows <= PLAYER_QUEUE_PART_ROWS {
        sync.queue = Some(queue);
        return vec![SessionMessage::Update(UpdateEnvelope {
            subscription,
            version,
            parts: 1,
            index: 0,
            payload: UpdatePayload::Player {
                sync: Box::new(sync),
                queue_parts: 0,
            },
        })];
    }
    let original = queue.original_queue.take();
    let part_count = rows.div_ceil(PLAYER_QUEUE_PART_ROWS);
    let parts = u32::try_from(part_count).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(part_count.saturating_add(1));
    out.push(SessionMessage::Update(UpdateEnvelope {
        subscription,
        version,
        parts: parts.saturating_add(1),
        index: 0,
        payload: UpdatePayload::Player {
            sync: Box::new(sync),
            queue_parts: parts,
        },
    }));
    for (chunk_index, chunk) in queue.queue.chunks(PLAYER_QUEUE_PART_ROWS).enumerate() {
        let index = u32::try_from(chunk_index.saturating_add(1)).unwrap_or(u32::MAX);
        let offset =
            u32::try_from(chunk_index.saturating_mul(PLAYER_QUEUE_PART_ROWS)).unwrap_or(u32::MAX);
        out.push(SessionMessage::Update(UpdateEnvelope {
            subscription,
            version,
            parts: parts.saturating_add(1),
            index,
            payload: UpdatePayload::PlayerQueuePart {
                offset,
                queue: chunk.to_vec(),
                // 原序只在第一片携带(整段一次性重建,避免逐片重复大载荷)。
                original_queue: (chunk_index == 0).then(|| original.clone()).flatten(),
            },
        }));
    }
    out
}

/// 把一份下载明细快照拆成头段 + 若干行片。
///
/// # Params:
///   - `subscription`: 目标订阅
///   - `version`: 该订阅的本次逻辑版本
///   - `rows`: 完整明细(调用方已按展示序排好)
///
/// # Return:
///   按发送顺序排列的消息。
pub fn fragment_download_detail(
    subscription: SubscriptionId,
    version: u64,
    rows: Vec<SongDownloadView>,
) -> Vec<SessionMessage> {
    let total = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    if rows.len() <= DOWNLOAD_DETAIL_PART_ROWS {
        return vec![SessionMessage::Update(UpdateEnvelope {
            subscription,
            version,
            parts: 1,
            index: 0,
            payload: UpdatePayload::DownloadsDetailSnapshot(rows),
        })];
    }
    let part_count = rows.len().div_ceil(DOWNLOAD_DETAIL_PART_ROWS);
    let parts = u32::try_from(part_count.saturating_add(1)).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(part_count.saturating_add(1));
    out.push(SessionMessage::Update(UpdateEnvelope {
        subscription,
        version,
        parts,
        index: 0,
        payload: UpdatePayload::DownloadsDetailHead { total },
    }));
    for (chunk_index, chunk) in rows.chunks(DOWNLOAD_DETAIL_PART_ROWS).enumerate() {
        let index = u32::try_from(chunk_index.saturating_add(1)).unwrap_or(u32::MAX);
        let offset = u32::try_from(chunk_index.saturating_mul(DOWNLOAD_DETAIL_PART_ROWS))
            .unwrap_or(u32::MAX);
        out.push(SessionMessage::Update(UpdateEnvelope {
            subscription,
            version,
            parts,
            index,
            payload: UpdatePayload::DownloadsDetailPart {
                offset,
                rows: chunk.to_vec(),
            },
        }));
    }
    out
}
