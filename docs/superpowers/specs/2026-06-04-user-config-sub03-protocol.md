# Sub03 — 协议演进:Event / request-id / 能力握手(Phase 1 前置)

> 主 spec:`2026-06-04-user-config-lua-design.md`(已定宪法)。本文只细化 §10 六条护栏的落地。
> 本文档按项目约定不纳入版本控制。

## 1. 范围与不做

**本 subspec 负责**(crate `mineral-protocol` + IPC 管线两端):

- 帧协议从「严格 1:1 顺序」升级为 **request-id 配对 + Event 同连接交错下推**(护栏 #2)。
- 引入 `Event` 推送 enum,把现有「轮询式推送」(`DrainTaskEvents` 里的 `TaskEvent::Notice`、每 tick 拉 `AudioSnapshot`/`PlayerSnapshot`/`DownloadProgress`)归并为统一 `Event::Toast` / `Event::PropertyChanged`(护栏 #1/#3)。
- 握手 `ClientInfo` / 能力协商 / 订阅集(护栏 #4)。
- `KeyTriggered{action, ctx}` 与 `KeyContext`(为 Phase 2 键转发铺协议,**本期只定形,不接执行**)。
- 队列 `entry_id + pos + version` 单调寻址协议字段(护栏 #5)。
- serde derive 边界,保证 codec 可换(bincode 今天 / JSON 将来,护栏 #1/#6)。
- 两端同仓同发版的可破坏性迁移步骤。

**本 subspec 不做**(交相邻 subspec / 后续 Phase):

- 不实现 daemon 侧 Lua VM、observe 分发、hooks 注册表(那是 `mineral-script` subspec)。
- 不实现 TUI 端的 `Action` 枚举统一重构、声明式 keymap 解析(那是 TUI/config subspec;本文只提供 `KeyTriggered`/`KeyContext` 协议形状供其消费)。
- 不定义 `Config` schema、loader、`default.lua`(config subspec)。
- 不实现属性树的「订阅即回放 + 末值合并」**语义**(那是 daemon 侧 `mineral-script` 实现;本文只定义 `PropertyChanged` 的**线格式**与 `prop` 命名空间约定)。
- 不做多 client fanout(serve 循环仍单 client;但帧协议为将来的交错下推就位)。
- 不动二进制旁路(封面仍走独立 `CoverFetcher`,护栏 #6 = 保持现状不内联)。

**与相邻 subspec 的边界**:本文产出的是 `mineral-protocol` 的**类型 + codec 帧** + serve 循环/RemoteClient 的**收发骨架**。`mineral-script` 消费 `Event` 的生产端、`PropertyChanged` 的属性源;TUI/config subspec 消费 `KeyTriggered`/`KeyContext`/`Event` 的客户端解码端。

## 2. 现状锚点(file:line,均已核对)

- 协议门面:`crates/mineral-protocol/src/lib.rs:1-22`。模块注释 `lib.rs:10` 明文「**当前不支持** 多路复用 / 异步推送」——本 subspec 即拆这条限制。
- `Request` / `Response` enum:`crates/mineral-protocol/src/message.rs:82-221`。`Response::Error(String)` 在 `message.rs:219-220`。
- codec(bincode + length-delimited):`crates/mineral-protocol/src/codec.rs:1-53`。`Framed<T>` = `TokioFramed<T, LengthDelimitedCodec>`(`codec.rs:15`);`send`/`recv` 是泛型 `T: Serialize`/`DeserializeOwned`(`codec.rs:26-53`),**与具体消息类型解耦**——这是 codec 可换的现有杠杆。
- 帧编码:`codec.rs:31` `bincode::serialize`,`codec.rs:51` `bincode::deserialize`。**位置式编码**(`codec.rs` 测试 `message.rs` 注释强调 bincode 位置式、字段不可重排)。
- serve 循环(单 client、严格 1:1):`crates/mineral-server/src/serve.rs:66-75`(`handle_connection` while-recv-dispatch-send),`serve.rs:36-41`(busy 拒第二 client),`serve.rs:83-165`(`dispatch`)。
- RemoteClient(客户端 worker,串行 round-trip):`crates/mineral-tui/src/runtime/remote.rs:82-108`(`worker` + `round_trip`)。`send_recv` 用 `std::sync::mpsc` 阻塞等单条 reply(`remote.rs:67-74`)。**关键约束**:worker 现在假设「发一条 = 收一条且即为本条 reply」,引入交错 Event 后此假设破裂。
- TUI 轮询点(每 TICK):`crates/mineral-tui/src/app.rs:192-208`(`drain_task_events` / `audio_snapshot` / `player_snapshot` / `task_snapshot` / `download_progress` 五连拉)。
- 既有「推送」雏形:`TaskEvent::Notice{text}`(`crates/mineral-task/src/event.rs:67-72`),client 在 `app.rs:384-395` 经 `DrainTaskEvents` 拉到后 `notifications.flash_text`。下载进度雏形 `DownloadProgress`(`message.rs:13-50`)经每 tick `download_progress()` 拉(`app.rs:207-208`)。
- 队列协议现状:`PlayerSnapshot{ queue: Vec<Song>, queue_sel: usize, original_queue }`(`crates/mineral-protocol/src/player.rs:130-159`)。**无 entry_id、无 version**——寻址靠整列 `Vec<Song>` + `usize` 位置 + `SongId`。`SetQueue{queue, target_id}`(`message.rs:129-134`)用 `target_id: SongId` 寻址当前位。client 不灌 `queue_sel`(`app.rs:286`)。
- KeyEvent 入口:`app.rs:397` `handle_event`(键散在各 `handle_*_key`,主 spec §7 已记 `app.rs:463,474` 视图裁决、`popup/component.rs:79-88` 双 `OverlayAction`)。
- e2e 串行组:`.config/nextest.toml` `[test-groups] daemon-e2e`,override `binary(daemon_lifecycle)`;e2e 在 `crates/mineral/tests/daemon_lifecycle.rs`。
- codec 单测:`crates/mineral-protocol/tests/codec.rs`(round-trip + proptest `request_bincode_roundtrip`,`codec.rs:321-329`)。

## 3. 新增 / 修改文件清单

守「单文件 ≤ 800 行(不含 `#[cfg(test)]`)、`mod.rs`/`lib.rs` 不写逻辑、`pub` 必带文档」。

| 文件 | 动作 | 职责 | 预估规模 |
|---|---|---|---|
| `crates/mineral-protocol/src/lib.rs` | 改 | 重写模块注释(去掉「不支持推送」);新增 `pub use` 导出(`Frame`/`Event`/`ClientInfo`/`ServerHello`/`Capability`/`Subscription`/`KeyTriggered`/`KeyContext`/`ViewKind`/`PropName`/`PropValue`/`RequestId`/`QueueEntry`)。**不写逻辑**。 | +15 行 |
| `crates/mineral-protocol/src/frame.rs` | 新增 | `RequestId`(newtype `u64`)、`Frame` enum(`Request{id,req}` / `Response{id,resp}` / `Event(Event)` / `Hello(ServerHello)` / `Handshake(ClientInfo)`)。同连接交错下推的**唯一 wire 顶层类型**。 | ~120 行 |
| `crates/mineral-protocol/src/event.rs` | 新增 | `Event` enum(`Toast` / `PropertyChanged` / `QueueChanged` / `TrackFinished` / `DownloadCompleted` …);`ToastKind`;`PropName`(属性命名空间 newtype,仿 SourceKind)、`PropValue`。 | ~200 行 |
| `crates/mineral-protocol/src/handshake.rs` | 新增 | `ClientInfo`(私有字段 + `#[non_exhaustive]` + builder + getters)、`ClientKind`、`ServerHello`、`Capability`、`Subscription`、`PROTOCOL_VERSION` 常量。 | ~180 行 |
| `crates/mineral-protocol/src/key.rs` | 新增 | `KeyTriggered{action, ctx}`、`KeyContext`(私有字段 + builder + getters)、`ViewKind`。**本期只定形**。 | ~160 行 |
| `crates/mineral-protocol/src/queue.rs` | 新增 | `QueueEntry{entry_id, song}`、`EntryId`(`define_uuid!` 或 newtype)、`QueueVersion`(单调 `u64` newtype)、`QueueAddr`(pos+id 双寻址 enum)。 | ~140 行 |
| `crates/mineral-protocol/src/message.rs` | 改 | `Request` 增 `Subscribe(Vec<Subscription>)` / `Unsubscribe`;`SetQueue` 增 `version` 字段(可选迁移);其余不动。 | +30 行 |
| `crates/mineral-protocol/src/player.rs` | 改 | `PlayerSnapshot` 增 `queue_version: QueueVersion`、`queue: Vec<QueueEntry>`(由 `Vec<Song>` 升级)。 | +20 行改 |
| `crates/mineral-protocol/src/codec.rs` | 改 | `send`/`recv` 保持泛型不动;新增 `pub type EventSink` 别名说明 + 文档明确「Frame 是唯一过 codec 的顶层类型」。 | +10 行 |
| `crates/mineral-protocol/tests/frame.rs` | 新增 | Frame / Event / 握手 / KeyContext / QueueEntry 的 round-trip + proptest(bincode 与 serde_json 双 codec)。 | ~250 行 |
| `crates/mineral-server/src/serve.rs` | 改 | `handle_connection` 改为读 `Frame`、握手协商、并发 reply + event 下推(split sink/stream)。 | +120 行改 |
| `crates/mineral-tui/src/runtime/remote.rs` | 改 | worker 改为 id 配对(`FxHashMap<RequestId, reply_tx>`)、Event 经独立 channel 推给 app;握手发送。 | +130 行改 |
| `crates/mineral-tui/src/app.rs` | 改 | tick 改为 drain event channel(替代部分轮询);`apply_event`。 | +60 行改 |

> serve.rs 现 196 行、remote.rs 现 377 行(含 test 块)。改动后 serve.rs 主体逼近上限,**若超 500 行预警**则把 event 下推 split 抽 `serve/pump.rs` 子模块(本 subspec 预留该拆分点,不强制)。

## 4. 关键类型与签名

### 4.1 Frame:同连接交错下推的顶层 wire 类型

```rust
//! crates/mineral-protocol/src/frame.rs

use serde::{Deserialize, Serialize};

use crate::{ClientInfo, Event, Request, Response, ServerHello};

/// 单调请求标识。client 自增分配,server 在对应 [`Frame::Response`] 原样回带,
/// 用于在交错的 Event 流里把 reply 配对回发起方的请求。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RequestId(u64);

impl RequestId {
    /// 用裸值构造(client 侧自增计数器单入口)。
    #[must_use]
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// 裸值,日志 / 调试用。
    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

/// IPC 连接上**唯一**过 codec 的顶层帧。一条连接上 client→server 与 server→client
/// 双向都发 `Frame`;`Request`/`Response` 经 [`RequestId`] 配对,`Event` 可在任意
/// 时刻交错下推(不占 reply 槽,client 永远单连接)。
///
/// codec 无关:本类型只 derive `Serialize`/`Deserialize`,bincode(今)与 JSON
/// (将来)切换不改本定义。
#[derive(Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Frame {
    /// client → server:握手首帧(连接建立后第一帧,先于任何 [`Frame::Request`])。
    Handshake(ClientInfo),

    /// server → client:握手应答(协商结果 + 协议版本 + 生效订阅)。
    Hello(Box<ServerHello>),

    /// client → server:带 id 的请求。
    Request {
        /// 配对标识。
        id: RequestId,
        /// 请求体(沿用既有 [`Request`])。
        req: Request,
    },

    /// server → client:带 id 的应答,`id` 原样回带发起的 [`Frame::Request`]。
    Response {
        /// 与对应请求相同的标识。
        id: RequestId,
        /// 应答体(沿用既有 [`Response`])。
        resp: Box<Response>,
    },

    /// server → client:主动推送,不配对 id。
    Event(Event),
}
```

> `Response` 现为 `Debug + Serialize + Deserialize`(无 `Clone`);`Box<Response>` 沿用 `message.rs:194` 的减体积手法。`Frame` 故意不实现 `PartialEq`(沿用 `Request`/`Response` 既有约定,测试比 `Debug`)。

### 4.2 Event:归并现有推送

```rust
//! crates/mineral-protocol/src/event.rs

/// server → client 主动推送。归并三类既有「轮询式推送」:
/// `TaskEvent::Notice`(toast)、`AudioSnapshot`/`PlayerSnapshot` 标量变更
/// (→ `PropertyChanged`)、`DownloadProgress` 完成(→ `DownloadCompleted`/`Toast`)。
///
/// 设计原则(主 spec D7):**状态值变更一律走 [`Event::PropertyChanged`]**,
/// 不为标量造 bespoke 事件;只有非属性化生命周期事故(`TrackFinished` 带 reason)留独立 variant。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Event {
    /// 瞬时提示。`id` 相同则替换不堆叠(nvim `msg_show` 语义)。
    Toast {
        /// 视觉级别。
        kind: ToastKind,
        /// 单行人读文本。
        content: String,
        /// 去重 / 替换键;`None` 表示一次性 flash(不替换既有)。
        id: Option<String>,
    },

    /// 属性树某项变更。observe 的协议面;「订阅即回放 + 末值合并」语义在 daemon 侧实现。
    PropertyChanged {
        /// 属性名(命名空间化,见 [`PropName`])。
        prop: PropName,
        /// 新值。
        value: PropValue,
    },

    /// 队列结构变更(增删 / 重排 / 替换);带新版本号,client 据此决定是否重拉快照。
    QueueChanged {
        /// 变更后的单调版本。
        version: crate::QueueVersion,
    },

    /// 一首歌播完(非属性化事故,必带 reason)。
    TrackFinished {
        /// 关联歌曲 id。
        song_id: mineral_model::SongId,
        /// 结束原因。
        reason: FinishReason,
    },

    /// 下载完成(单曲)。承接 `DownloadProgress.result_seq` 增长语义的离散化。
    DownloadCompleted {
        /// 关联歌曲 id。
        song_id: mineral_model::SongId,
    },

    /// per-song 持久 KV(`store.*`)变更的**粗粒度**通知(MPD sticker 子系统风格)。
    ///
    /// **本变体由 sub05(Phase 2)的 PR 追加**(归属本 event.rs 文件、sub03 owner;
    /// 在 sub05 PR 中作为"追加变体,经 sub03 owner 文件"落地)。理由:observe 的
    /// "订阅即回放 + 末值合并"语义只对**有限属性树**(`player.*` / `queue.*` 等)成立;
    /// per-song KV 是**开放命名空间的数据库**,不是属性树,故不进 `PropertyChanged`,
    /// 走独立 `StoreChanged`——只报 song_id + key,接收方按需重读(主 spec §8 范围注)。
    StoreChanged {
        /// 变更的歌曲 id。
        song_id: mineral_model::SongId,
        /// 变更的 KV 键(开放命名空间或一等字段名)。
        key: String,
    },
}

/// Toast 视觉级别。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToastKind {
    /// 普通信息。
    Info,
    /// 警告。
    Warn,
    /// 错误。
    Error,
}

/// 曲目结束原因(对齐主 spec §6 `track_finished` reason 域)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishReason {
    /// 自然播完。
    Eof,
    /// 用户跳过。
    Skip,
    /// 解码 / 网络错误。
    Error,
    /// 用户停止。
    Stop,
}
```

`PropName` / `PropValue`(observe 协议面,与 Lua `mineral.get`/`observe` 同形):

```rust
/// 属性树键。仿 [`mineral_model::SourceKind`]:newtype + 关联常量,**开放**
/// (插件 / 脚本经 `from_static` 铸造)。身份只认内部 `&'static str`,故 `Copy`、强类型。
///
/// serde 序列化只写字符串名,反序列化按名解析回常量、未知名 intern。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PropName(&'static str);

impl PropName {
    /// 当前在播歌(`player.song`)。
    pub const PLAYER_SONG: Self = Self("player.song");
    /// 播放态(`player.state`)。
    pub const PLAYER_STATE: Self = Self("player.state");
    /// 音量(`player.volume`)。
    pub const PLAYER_VOLUME: Self = Self("player.volume");
    /// 进度 ms(`player.position`)。
    pub const PLAYER_POSITION: Self = Self("player.position");
    /// 播放模式(`player.mode`)。
    pub const PLAYER_MODE: Self = Self("player.mode");
    /// 队列长度(`queue.length`)。
    pub const QUEUE_LENGTH: Self = Self("queue.length");

    /// 名字裸值。
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.0
    }
}

/// 属性值的协议化载荷。**已裁决**用窄变体(Bool/Int/Str/None),**不**引入
/// `serde_json::Value` 兜底——本期可观测的领域态(volume / position / state / mode /
/// queue.length 等标量)都能塌进这几个变体。整首 `Song` 的传输(`player.song`)**推迟到
/// Phase 2**(届时若需结构化载荷再评估专门表示),本期 `player.song` 用 `Str`(歌曲 id 的
/// `qualified()`)或 `None` 表达,client 按需另拉详情。这样 `PropValue` 不跟领域类型膨胀,
/// protocol crate 也不引入 `serde_json` prod 依赖。
///
/// `Float(f64)` 留作逃生口——Phase 1 六属性无浮点量(volume 为百分比整数、position 整秒
/// 节流,更细可用 Int 装毫秒),且 f64 无 Eq(仅 PartialEq,NaN 自反性陷阱),若未来
/// `PropValue` 需要去重/索引等 Eq/Hash 场景会被堵死(`PropName`(键)的 Eq/Hash 不受影响);
/// 两端同仓同发版,将来首个真浮点属性出现时追加变体零迁移成本。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PropValue {
    /// 布尔(如 player.state 播/停)。
    Bool(bool),
    /// 整数(如 volume / position / queue.length)。
    Int(i64),
    /// 字符串(如 player.mode 的 label、player.song 的 qualified id)。
    Str(String),
    /// 缺省 / 空(如无在播歌)。
    None,
}
```

> `PropName` 的 serde:手写 `Serialize`(写 `self.0`)+ `Deserialize`(`from_static` intern),与 `SourceKind` 同款做法(见 CLAUDE.md「SourceKind 序列化只写 name」)。

### 4.3 握手:ClientInfo / 能力 / 订阅集

```rust
//! crates/mineral-protocol/src/handshake.rs

/// 协议版本。两端同仓同发版(主 spec §10),不匹配时 server 在 [`ServerHello`]
/// 里回 `accepted = false`,client 据此提示「请升级」并干净退出。
pub const PROTOCOL_VERSION: u32 = 1;

/// client 类型,影响 server 默认订阅集与字段窄化。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientKind {
    /// 交互式 TUI(全订阅:property + toast + queue)。
    Tui,
    /// 一次性 CLI(如 `mineral status`,最小订阅 / 不订阅 event)。
    Cli,
    /// 远期 frontend(web / nvim)占位;本期不发,仅协议留位。
    Frontend,
}

/// client → server 握手首帧。私有字段 + builder + getters(对外配置 struct 约定)。
#[derive(Clone, Debug, Serialize, Deserialize, TypedBuilder, Getters)]
#[non_exhaustive]
pub struct ClientInfo {
    /// client 协议版本。
    #[getter(copy)]
    protocol_version: u32,

    /// client 类型。
    #[getter(copy)]
    kind: ClientKind,

    /// 期望订阅集(空 = 用 server 按 `kind` 给的默认)。
    #[builder(default)]
    subscriptions: Vec<Subscription>,
}

/// server → client 握手应答。
#[derive(Clone, Debug, Serialize, Deserialize, TypedBuilder, Getters)]
#[non_exhaustive]
pub struct ServerHello {
    /// 协商是否成功(版本兼容)。`false` 时其余字段无意义,client 应退出。
    #[getter(copy)]
    accepted: bool,

    /// server 协议版本。
    #[getter(copy)]
    protocol_version: u32,

    /// server 支持的能力集(client 据此知道哪些 Request 可用)。
    capabilities: Vec<Capability>,

    /// 实际生效的订阅集(可能窄于 client 请求)。
    subscriptions: Vec<Subscription>,

    /// daemon 已注册的**自定义动作名集合**(主 spec §7:TUI 据此知道哪些键转发)。
    /// 本期 daemon VM 未落地,恒为空 vec;协议留位。
    custom_actions: Vec<String>,
}

/// 订阅类别(client 想收哪类 Event)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Subscription {
    /// 属性变更(observe 面)。
    Property,
    /// toast 提示。
    Toast,
    /// 队列结构变更。
    Queue,
}

/// server 能力位。client 据此做字段窄化 / 功能开关,规避 MPD 弱协商。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Capability {
    /// 支持 Event 推送(本期恒有)。
    EventPush,
    /// 支持自定义动作转发(Phase 2 daemon VM 就位后置真)。
    CustomActions,
    /// 支持队列 pos+id 双寻址。
    QueueAddressing,
}
```

### 4.4 KeyTriggered / KeyContext(本期只定形)

> **命名已裁决变更(2026-06-04,实现时以此为准)**:载荷里没有任何「键」——只有动作名 + 上下文;「键」是 TUI 触发动作的本地成因,不是消息内容。把 TUI-only 概念写进协议契约会让 web/nvim client 经按钮/命令触发同一具名动作时语义别扭。故统一改名为 client 中立词:
> - `KeyTriggered` → **`InvokeAction`**
> - `KeyContext` → **`ActionContext`**
> - `crates/mineral-protocol/src/key.rs` → **`action.rs`**
> - `Request::KeyAction(KeyTriggered)` → **`Request::InvokeAction(InvokeAction)`**
>
> `ViewKind` 不改(它本就声明「协议无关于具体 TUI 布局」)。本文其余处及 sub05 出现旧名,一律按此映射读;形状、字段、builder 约定不变。

```rust
//! crates/mineral-protocol/src/key.rs

/// TUI → daemon:一个**自定义动作名**被某键触发,daemon VM 执行 `fn(ctx)`。
/// 内建动作不走此路(TUI/daemon 各自本地实现);仅 daemon 注册的动作名经此转发。
///
/// 本期(sub03)仅定义协议形状并过 codec;实际转发执行在 Phase 2 daemon VM 落地。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyTriggered {
    /// daemon VM 里注册的动作名(`mineral.action(name, fn)` / `mineral.bind`)。
    pub action: String,
    /// 按键瞬间的只读视图快照。
    pub ctx: KeyContext,
}

/// 按键瞬间的只读视图上下文。视图态本体不进 daemon(主 spec D5),只传这一份快照。
/// 私有字段 + builder + getters。
#[derive(Clone, Debug, Default, Serialize, Deserialize, TypedBuilder, Getters)]
#[non_exhaustive]
pub struct KeyContext {
    /// 当前视图。
    #[getter(copy)]
    #[builder(default)]
    view: ViewKind,

    /// 当前选中歌(列表 / 浮层光标);无则 None。
    #[builder(default)]
    selected_song_id: Option<mineral_model::SongId>,

    /// 当前选中歌单;无则 None。
    #[builder(default)]
    selected_playlist_id: Option<mineral_model::PlaylistId>,

    /// 当前在播歌;无则 None。
    #[builder(default)]
    now_playing_id: Option<mineral_model::SongId>,
}

/// 视图类别(协议无关于具体 TUI 布局;只给脚本判断「在哪个面上按的」)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ViewKind {
    /// 歌单列表。
    #[default]
    Playlists,
    /// 曲目列表。
    Tracks,
    /// 队列浮层。
    Queue,
    /// 全屏播放。
    Fullscreen,
    /// 搜索输入。
    Search,
}
```

> `Request` 增一条转发入口:`Request::KeyAction(KeyTriggered)`(本期 server `dispatch` 收到回 `Response::Ok` 并 log「VM 未就位、忽略」,Phase 2 接 VM)。

### 4.5 队列 entry_id + pos + version

```rust
//! crates/mineral-protocol/src/queue.rs

/// 队列内单调版本号。任何结构变更(SetQueue / 增删 / 重排)递增;client 比对决定
/// 是否需要重拉,server 在 [`Event::QueueChanged`] 与 `PlayerSnapshot` 同带。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct QueueVersion(u64);

impl QueueVersion {
    /// 下一版本(server 端每次结构变更调用)。
    #[must_use]
    pub fn bump(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
    /// 裸值。
    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

/// 队列内**稳定句柄**:同一首歌可在队列出现多次,`SongId` 不足以寻址;`EntryId`
/// 在该 entry 生命周期内稳定,重排不变、删除即失效。脚本 API(`queue.jump/remove`)
/// 与 client 拖拽据此精确寻址。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntryId(u64);

impl EntryId {
    /// server 端分配(单调计数器,不复用)。
    #[must_use]
    pub fn new(value: u64) -> Self {
        Self(value)
    }
    /// 裸值。
    #[must_use]
    pub fn value(self) -> u64 {
        self.0
    }
}

/// 队列一项 = 稳定句柄 + 歌。`PlayerSnapshot.queue` 由 `Vec<Song>` 升级为 `Vec<QueueEntry>`。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueueEntry {
    /// 稳定句柄。
    pub entry_id: EntryId,
    /// 歌。
    pub song: mineral_model::Song,
}

/// pos+id 双寻址(护栏 #5)。优先 `Entry`(稳定);`Pos` 是宽容回退(client 只有
/// 位置时用,server 按当前 version 解释,版本错位则拒绝并回 toast)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueAddr {
    /// 按稳定句柄寻址(推荐)。
    Entry(EntryId),
    /// 按位置寻址(需带读到该位置时的 version 做乐观校验)。
    Pos {
        /// 位置。
        pos: usize,
        /// 读到该位置时的队列版本。
        seen_version: QueueVersion,
    },
}
```

## 5. 实现步骤(依赖顺序,可拆 PR)

> 两端同仓同发版,允许一次性破坏性切换(主 spec §10「两端同仓同发版,可破坏性迁移」)。**不引入兼容旧帧的过渡层**——同一个 PR 改协议 + 两端,CI 全绿即可。

1. **PR-A:协议类型落地(纯新增,无行为变更)**
   - 新增 `frame.rs` / `event.rs` / `handshake.rs` / `key.rs` / `queue.rs`,`lib.rs` 加导出。
   - `message.rs` 加 `Request::Subscribe/Unsubscribe/KeyAction`;`player.rs` 把 `PlayerSnapshot.queue` 升级为 `Vec<QueueEntry>` + 加 `queue_version`。
   - 写 `tests/frame.rs`(round-trip + proptest,双 codec)。
   - 此 PR 还**没接管线**:serve/remote 仍走旧 `Request`/`Response` 裸帧 → 编译期会因 `PlayerSnapshot.queue` 类型变更逼出两端跟改(见 6 测试 / 7 风险)。故 PR-A 与 PR-B 可能需合并提交以保 CI 绿;若要拆,PR-A 内 `PlayerSnapshot` 暂留 `Vec<Song>`、entry_id 在 PR-C 升级。
   - 验收:`cargo nextest run -p mineral-protocol` 绿、`cargo td -p mineral-protocol` 绿。

2. **PR-B:管线切到 Frame + 握手 + id 配对**
   - `codec.rs`:文档明确 `Frame` 是唯一顶层类型;`send`/`recv` 泛型不变(已天然支持 `Frame`)。
   - `serve.rs`:`handle_connection` 改为 (a) 先 `recv::<Frame>` 期待 `Handshake`,协商后回 `Hello`;(b) `Framed` split 成 sink/stream(`futures_util::StreamExt::split`);(c) 起一个 `tokio::sync::mpsc` 作为 event 通道,主循环 `select!` 读 client `Frame::Request` → spawn/await dispatch → 发 `Frame::Response{id}`,同时把 event 通道里的 `Event` 发成 `Frame::Event`。**dispatch 复用现有 `dispatch()` 不变**(只在外面包 id)。
   - `remote.rs`:worker 改为持 `FxHashMap<RequestId, std::sync::mpsc::Sender<Response>>`;发请求时分配 id 入表;收到 `Frame::Response{id}` 按表配对回 reply;收到 `Frame::Event` 推给独立的 `event_tx`(交 app 消费)。`RemoteClient::connect` 先发 `Handshake`、等 `Hello` 校验 `accepted`。
   - `app.rs`:tick 里 drain event channel(`apply_event`),`PropertyChanged`/`Toast` 进通知/状态;**保留**轮询作为兜底(本期 daemon 未产 PropertyChanged,先把通道接通,event 实际生产在 `mineral-script` subspec)。
   - 验收:daemon e2e（`daemon_lifecycle.rs`）绿;握手失败路径测试绿。

3. **PR-C:队列 entry_id 接管**
   - server 侧 `state.rs`/`player.rs`(相邻,本 subspec 不主改;协议提供 `QueueEntry`/`EntryId`/`QueueVersion`,server subspec 接);`SetQueue` 走 version。
   - 本 subspec 只保证协议字段就位 + round-trip 测试;server 端 entry_id 分配实现归 server/script subspec。

4. **PR-D(可选,Phase 2 前)**:`KeyAction` 在 serve dispatch 里接 `mineral-script`(本期回 `Response::Ok` + log no-op)。

## 6. 测试清单(对齐 docs/testing.md)

- **codec round-trip(新 `tests/frame.rs`)**:每个新类型(`Frame` 各 variant、`Event` 各 variant、`ClientInfo`/`ServerHello`、`KeyContext`、`QueueEntry`/`QueueAddr`/`QueueVersion`)走 bincode round-trip(Debug 等价,沿用 `codec.rs` 既有 `req_round_trips` 风格)。
- **双 codec 守卫**:同一组样本分别经 `bincode` 与 `serde_json` round-trip 断言等价——证明 codec 可换(护栏 #1/#6)。沿用 `mineral-server/tests/wire.rs:15-21` 的 `round_trip<T>` JSON helper 模式。
- **PropName serde 守卫**:`PropName::PLAYER_VOLUME` → JSON 应是裸字符串 `"player.volume"`;未知名反序列化 intern 不 panic(assert，不 unwrap）。
- **proptest 不变量**:`arb_frame()`(扩 `codec.rs:290` 的 `arb_request`)bincode 往返 Debug 恒等;`QueueVersion::bump` 严格递增(`v < v.bump()`)。
- **握手协商单测**:`ClientInfo`(版本不匹配)→ server 应回 `ServerHello{accepted: false}`;TUI 端 `connect` 收到 `accepted=false` 走干净退出分支。放 serve.rs `#[cfg(test)]` 或 remote.rs。
- **id 配对单测(remote.rs `#[cfg(test)]`)**:在 in-memory `tokio::io::duplex` 上,server 端**乱序**回两条 `Frame::Response`(id 倒序)+ 中间夹一条 `Frame::Event`,断言 client worker 把每条 reply 配对回正确的 `send_recv` 调用、event 进 event channel。**必须 `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`**(memory 教训:`send_recv` 同步阻塞当前线程,单线程 rt 下 worker task 拿不到线程死锁——`remote.rs:271,293` 既有先例)。
- **Event 下推 e2e**:进 daemon-e2e 串行组(`.config/nextest.toml`),`MINERAL_AUDIO_NULL=1`;起真 daemon、TUI 端连上后 server 主动推一条 `Event::Toast`,client 收到并 flash。新增到 `binary(daemon_lifecycle)` 覆盖范围或新 e2e binary(后者需在 nextest.toml 加 override 进 daemon-e2e 组)。
- **config check 快照不在本 subspec**(归 config subspec)。本 subspec 无 TUI 渲染快照新增;若 Event toast 改了通知渲染,补 `assert_snap!` 带中文 description(如「Event::Toast(Error) 渲染:红色单行,id 相同替换不堆叠」)。
- **`cargo clippy --workspace --all-targets -- -D warnings`** 全绿(新类型 `pub` 全带 `///`、无 unwrap/expect/as)。

## 7. 验收判据

1. `mineral-protocol` 新增五模块全部 `pub` 项有 `///`、配置 struct(`ClientInfo`/`ServerHello`/`KeyContext`)私有字段 + `#[non_exhaustive]` + builder + getters;无 unwrap/expect/panic/as;clippy `-D warnings` 绿。
2. 一条连接上 client 连发 N 个 `Frame::Request`、server 交错下推 `Frame::Event`,client 仍能按 id 把每条 `Response` 配对回正确发起方(乱序 reply 测试绿)。
3. 握手:版本匹配 → `accepted=true` + 生效订阅;不匹配 → `accepted=false`,TUI 干净退出。
4. `Frame`/`Event`/`PropValue` 等所有 wire 类型分别经 bincode 与 serde_json round-trip 等价(codec 可换守卫绿)。
5. 现有 daemon e2e（`daemon_lifecycle.rs`)在切到 Frame 后仍绿(单 client req/resp 行为不回归)。
6. 既有 `TaskEvent::Notice` 下载提示路径在归并后行为不变(下载完成仍 flash)。**已裁决**:**保留 `TaskEvent::Notice`,直至 daemon VM(sub04)产 `Event` 后再退役**(避免一次改动面过大),不在本 subspec 强行归并到 `Event::Toast`/`DownloadCompleted`。

## 8. 风险

| 风险 | 缓解 |
|---|---|
| `Frame` 切换 = 破坏性帧变更,旧 client/新 server 无法互通 | 两端同仓同发版(主 spec §10);不做兼容层,一个 PR 改协议 + 两端 + e2e 守。版本号 `PROTOCOL_VERSION` 在握手显式拒绝错配,给出「请升级」提示而非乱码崩溃。 |
| RemoteClient worker 由「发一收一」改 id 配对,死锁回归 | id 配对测试强制 `multi_thread` rt(memory 教训);保留 `send_recv` 阻塞语义不变,仅 worker 内部从「单 reply」改「按 id 查表 reply」。 |
| serve.rs split sink/stream 后并发写 sink 需要锁 | event 下推与 response 下推统一经一个 `mpsc` → 单 writer task 串行写 sink,杜绝并发写;dispatch 在 spawn task 里算完把结果送进同一 mpsc。 |
| `PlayerSnapshot.queue` 由 `Vec<Song>` 改 `Vec<QueueEntry>` 牵动 server `state.rs`/`player.rs` 与 TUI `app.rs:285` | 跨 subspec 接口(见 interfaces);若 PR-A 想纯协议,先留 `Vec<Song>`,entry_id 升级延到 PR-C 与 server subspec 同步。 |
| `PropValue` 是否引入 `serde_json` 依赖 | **已裁决**:窄变体(Bool/Int/Str/None)覆盖本期全部属性,**不**加 `Json` 变体、**不**引 serde_json 到 protocol prod;`player.song` 整首传输推迟 Phase 2。protocol crate 保持无 serde_json prod 依赖(双 codec 测试的 JSON helper 仍是 dev-dep)。 |
| `serde(deny_unknown_fields)` 与 `#[non_exhaustive]` enum 跨版本演进 | 本期两端同版,无需向前兼容;远期加 variant 时旧 client 遇未知 Event variant,bincode 位置式会解码失败 → 届时再引入 codec 协商或 `#[serde(other)]` 兜底,记入远期。 |
| serve.rs 改后逼近 800 行 | 预留 `serve/pump.rs` 拆分点(event 下推 split 抽出);>500 行预警即拆。 |

## 9. 裁决记录(交叉审查收敛)

本 subspec 是协议类型的 **canonical owner**;下列类型以本文为准,sub05(Phase 2)只**消费**、不重定义:

- **`KeyTriggered { action, ctx }` / `KeyContext`(私有字段 + TypedBuilder + Getters + `#[non_exhaustive]`)/ `ViewKind`(Playlists/Tracks/Queue/Fullscreen/Search 5 变体)**:归 `key.rs`(§4.4),本文已定形。sub05 的 §4.1 旧形状(裸 DTO、字段名 `context`、`KeyView` 二值枚举)已**作废**,改为消费本文形状。
- **`EntryId`(`queue.rs`,私有 `u64`)**:归 `queue.rs`(§4.5),sub05 的 `QueueEntryId` 已作废,统一用 `EntryId`。
- **`Request::KeyAction(KeyTriggered)`**:转发变体名以此为准(§4.4 末),sub05 的 `Request::KeyTriggered` 已作废。
- **`Event::StoreChanged { song_id, key }`**:已裁决新增独立变体(§4.2),由 sub05 PR 追加进本 owner 文件;store 写变更**不**走 `PropertyChanged`(见 §4.2 变体文档与主 spec §8 范围注)。

其余已裁决项:

- **`PropValue` 用窄变体**(Bool/Int/Str/None),不引 `serde_json::Value` 兜底;整首 `Song` 传输**推迟 Phase 2**(§4.2)。
- **`TaskEvent::Notice` 保留**,daemon VM(sub04)产 `Event` 后再退役(§7.6)。
- **`PlayerSnapshot.queue` 升级 `Vec<QueueEntry>` 延到 PR-C**:PR-A 纯协议阶段 `PlayerSnapshot.queue` 暂留 `Vec<Song>`(避免 PR-A 强制牵动 server/TUI),entry_id 升级在 PR-C 与 server subspec 同步(§5 PR-A/PR-C 已述)。
- **Event e2e 先扩 `daemon_lifecycle`**:本期 Event 下推 e2e 先并入既有 `binary(daemon_lifecycle)` 覆盖(§6),待事件面长大再拆独立 e2e binary(届时在 `.config/nextest.toml` 加 override 进 daemon-e2e 组)。
- **Phase 1→2 过渡保留快照轮询双轨**:Event 推送为**增强**、每 tick 轮询快照为**兜底**(匹配项目降级哲学);本期 daemon 未产 `PropertyChanged`,先接通通道,轮询仍是权威值来源(§5 PR-B `app.rs` 保留轮询)。
