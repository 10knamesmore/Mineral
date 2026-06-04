# Sub05 — 动作生态:键转发 + queue/library/store/timer + 热重载(Phase 2)

> 主设计文档:[`2026-06-04-user-config-lua-design.md`](2026-06-04-user-config-lua-design.md)(已定宪法)。
> 本 subspec 只细化 Phase 2 的落地,不重辩主文档决策。按项目约定不纳入版本控制。

## 1. 范围与不做

### 范围(对应主文档 §15 Phase 2 + §9 P2 行)

1. **自定义动作生态**:`mineral.action(name, fn)` 注册表 + `mineral.bind(key, fn)` 语法糖;
   握手时把"自定义动作名集合"下发给 TUI;TUI 侧 `KeyContext` 采集 + `KeyTriggered` 发送。
2. **queue API**:`queue.list/jump/add/remove`,pos + entry-id 双寻址落到 Lua(主文档 §10.5)。
3. **library API**:`library.playlists/tracks/love`,映射到现有 channel 能力。
4. **store API**:`store.get/set/inc(song_id, key)` per-song 持久 KV(mineral-persist 新表);
   内建一等字段 `local_play_count` / `rating` / `last_played`;开放 key 命名空间约定。
5. **timer API**:`timer.after/every` 脚本线程定时器,返回 timer handle(`stop`/`kill`/`resume`)。
6. **热重载**:`notify` watch config.lua → 重 eval 到新 VM → 注册表原子换 → 声明切片 diff 推送 →
   TUI 应用覆盖。
7. **player 级播放失败信号 + `track_finished("error")`**:mineral-server 把"SongUrl 解析失败 /
   解码失败"从 scheduler 的 `TaskOutcome::Failed` 提升出一条 **player 级播放失败信号**,经 sub04 的
   `notify_track_finished(song, Error)` 钩子喂进 `ScriptEvent::TrackFinished`,兑现 sub04 §6/§9
   标注的 `Error` reason 已知缺口与主 spec §6 的 reason 分期注(`mineral.on("track_finished", fn)`
   的 `reason="error"` Phase 2 落地)。详见 §4.8 / PR-K。

### 不做(边界)

- **不做** Phase 0/1 的奠基件:`mineral-config` loader、`default.lua`、强类型 `Config`、深合并、
  四承重墙最小集(player/get/observe/on/ui.toast/log)、看门狗线程、`mineral-script` crate 的
  骨架与脚本线程模型 —— 这些是 **sub01(config loader)/sub04(daemon VM)** 的产物,本 subspec 在其之上**追加** API。
- **不做** Phase 3 强力位:`hook("before_play"/...)` 同步拦截、`emit/on_message`、`spawn`、
  `library.search` 异步。
- **不做** `Action` 枚举统一前置重构本身(那是 Phase 0 的独立 PR,sub00 范围);本 subspec
  **消费**其产物(假设 TUI 已有中央 `Action` 枚举与声明式 keymap 解析)。
- **不碰**协议 transport / codec / 握手骨架的引入(单连接交错下推、request-id 配对、
  `ClientInfo` 能力协商):这些由 sub03 在 Phase 1 落地,本 subspec **扩展**其握手能力字段
  (server 填 "自定义动作名集合" 进 sub03 的 `ServerHello.custom_actions`)、`Request`(追加
  `KeyAction` / queue / store 变体)与事件枚举(经 sub03 owner 文件**追加 `Event::StoreChanged`**);
  `KeyContext`/`KeyTriggered`/`EntryId`/`ViewKind` 一律**复用 sub03 定义,不重定义**。
- **不做(记录在案的非计划项)Song 线上整体传输**:remote client 经现有请求 / 响应通道已能拿到完整
  `Song`(搜索 / 详情 / 队列快照等),observe 的 `player.song` 只携带 `qualified()` id 是合理终态
  ——接收方按需以 id 重读。**这条从"推迟"降级为非计划项**:无新增需求不动,按需再议(不占 PR、不进
  分期目录)。

## 2. 现状锚点(精确 file:line)

### 键路径(TUI)

- 顶层分发 `App::handle_key` — `crates/mineral-tui/src/app.rs:407`;Ctrl-C 强退 `:409`、
  转场吞键 `:419`、浮层优先 `:424`、搜索态 `:434`、全屏 `z` `:441`、Tab/q/t `:447-461`、
  `/` 搜索 `:463`、playback 键 `:469`、view 路由 `:474-479`。
- 视图键处理:`handle_playback_key` `:556`、`handle_playlists_key` `:621`、
  `handle_library_key` `:688`(love `f` `:736`、download `d` `:745`、Enter set_queue+play `:720`)。
- 浮层意图枚举 `OverlayAction` — `crates/mineral-tui/src/components/popup/component.rs:79`
  (`Quit` / `CloseTop` / `PlayQueueIndex(usize)`)。
- `App` 持 `state: AppState` + `client: Arc<dyn Client>` — `app.rs:59,75`。

### KeyContext 取数字段(AppState)

- `View` 枚举(`Playlists` / `Library`)— `crates/mineral-tui/src/runtime/state.rs:38`;
  `AppState.view` `:64`、`sel_playlist` `:96`、`sel_track` `:103`、`current: Option<Song>` `:111`、
  `queue: Vec<Song>` `:123`、`playlists: Vec<PlaylistView>` `:78`。
- 选中歌单 id:`state.filtered_playlists().get(sel_playlist).map(|p| p.data.id.clone())`
  (`app.rs:655` 既有取法);选中曲目:`state.filtered_tracks().get(sel_track)`(`app.rs:721`)。
- now_playing id:`state.current.as_ref().map(|s| s.id.clone())`。

### Client trait / 协议 / serve

- `Client` trait — `crates/mineral-server/src/client.rs:89`(全 sync 方法;`connected()` 默认 `true`)。
- `RemoteClient`(跨进程实现)— `crates/mineral-tui/src/runtime/remote.rs:39`;
  `send_recv` 严格 1:1 round-trip `:67`;worker `:82`。
- `Request` / `Response` enum — `crates/mineral-protocol/src/message.rs:82,176`(**目前无 Event 推送**)。
- serve dispatch — `crates/mineral-server/src/serve.rs:83`;`on_connect` 钩子 `:21,42`;
  单 client busy `:36`。
- codec bincode + length-delimited — `crates/mineral-protocol/src/codec.rs:18,26,42`。

### store / library / persist

- `ServerStore` 门面 — `crates/mineral-persist/src/server_store.rs:19`;`scope(source)` `:88`、
  `pool()` `:74`、`disabled()` `:63`;`open()` 建表 `:50`。
- `NamespaceStore`(per-source 视图)— `crates/mineral-persist/src/db/namespace.rs:68`;
  既有 `record_play` `:249`、`set_loved` `:339`、`query_stats` `:304`、`SongStats` `:16`
  (含 `play_count` / `last_played_at` / `loved`)。
- SQL schema — `crates/mineral-persist/src/db/schema.sql`(`song_stats` 表已有
  `play_count` / `loved_at` / `last_played_at`);`ensure_schema` — `db/schema.rs:17`。
- library 能力:`MusicChannel::my_playlists` / `songs_in_playlist` / `liked_song_ids` /
  `set_loved` — `crates/mineral-channel/core/src/lib.rs:84,51,97,109`。
- `PlayerCore`:`channel_for(source)` `crates/mineral-server/src/player.rs:173`、
  `persist()` `:239`、`spawn_on_played` `:501`(play 统计落点,store 变更事件的天然挂点)。
- channel `on_played` 调 `record_play` / `push_history` —
  `crates/mineral-channel/netease/src/channel.rs:333,340`。

### 路径 / id

- `mineral_paths::config_dir()` — `crates/mineral-paths/src/lib.rs:18`(热重载 watch 目标)。
- `SongId { namespace, value }`,`new` / `value()` / `qualified()` — 由
  `mineral_macros::define_id!` 生成(`crates/mineral-model/src/ids.rs:5`);`qualified()` =
  `namespace:value` 全局唯一,**store KV 主键 + Lua 侧 song_id 字符串句柄用它**。

## 3. 新增/修改文件清单

> 守约定:单文件 ≤ 800 行(不含 `#[cfg(test)]`),`mod.rs`/`lib.rs` 不写逻辑。
> `mineral-script` crate 的 `lib.rs` 与脚本线程骨架由 sub03/04 落;本 subspec 在其
> 已存在的模块树下**追加文件**,不重写骨架。

### mineral-protocol(增量)

| 文件 | 职责 | 规模 |
|---|---|---|
| `src/key.rs`(**sub03 已建,本 subspec 仅复用**) | `KeyContext` / `KeyTriggered` / `ViewKind` 由 sub03 定形(私有字段 + builder + getters + `#[non_exhaustive]`,`ViewKind` 5 变体);**本 subspec 不重定义**,只 `use`(已裁决,见 §4.1 / §7);键字符串规范化(`"space"`/`"S"` ↔ crossterm)在 TUI 侧 | ~0 行(复用) |
| `src/event.rs`(sub03 已建,**追加** variant) | **追加 `Event::StoreChanged { song_id, key }`**(已裁决:store 写变更走独立粗粒度事件,非 `PropertyChanged`;变体归 sub03 owner 文件,本 subspec PR 中追加,注明"追加变体,经 sub03 owner 文件") | +~15 行 |
| `src/message.rs`(改) | `Request` 追加 `KeyAction(KeyTriggered)`(变体名沿用 sub03)、queue 双寻址(`QueueJump`/`QueueAdd`/`QueueRemove`,带 sub03 的 `EntryId`)、`StoreGet/Set/Inc`;`Response` 追加 `StoreValue`、`QueueSnapshot`(带 version + entry-id);握手回执的 `custom_actions: Vec<String>` 在 sub03 的 `ServerHello` 上(sub03 已建,本期由 server 填值下发) | +约 120 行(逼近,必要时拆 `src/store_wire.rs`) |
| `src/lib.rs`(改) | 仅 `pub use` 追加 | +4 行 |

### mineral-script(在 sub03/04 骨架上追加)

| 文件 | 职责 | 规模 |
|---|---|---|
| `src/api/action.rs`(新) | `mineral.action(name, fn)` 注册表 + `mineral.bind(key, fn)` 语法糖(匿名 action + 声明切片 keys 追加);注册表 `ActionRegistry`(name → `RegistryKey`) | ~220 行 |
| `src/api/queue.rs`(新) | `queue.list/jump/add/remove` Lua 绑定;经命令通道投递给 daemon player,出参做 entry-id 编织 | ~200 行 |
| `src/api/library.rs`(新) | `library.playlists/tracks/love` Lua 绑定;映射 channel 能力(经命令通道走 player) | ~190 行 |
| `src/api/store.rs`(新) | `store.get/set/inc(song_id, key)` Lua 绑定;内建一等字段读路由到 `song_stats`,开放 key 走新 `song_kv` 表;变更后投递 **`Event::StoreChanged { song_id, key }`**(已裁决:独立粗粒度事件,非 `PropertyChanged`) | ~230 行 |
| `src/api/timer.rs`(新) | `timer.after/every` 脚本线程定时器 + `Timer` handle(`stop`/`kill`/`resume`);用脚本线程的 tick 心跳驱动(不另起 tokio) | ~200 行 |
| `src/reload.rs`(新) | `notify` watcher → 重 eval → 新 VM 注册表原子换 → 声明切片 diff → 推送;debounce | ~240 行 |
| `src/registry.rs`(新或在 sub03 内) | `ActionRegistry` / store dispatch / timer 表的承载结构,归脚本线程所有 | ~180 行 |
| `src/api/mod.rs`(改,sub03 已有) | 仅 `mod` 声明追加(不写逻辑) | +6 行 |

### mineral-persist(增量)

| 文件 | 职责 | 规模 |
|---|---|---|
| `src/db/song_kv.rs`(新) | `SongKvStore`:开放命名空间 KV(`store.get/set/inc` 的开放 key 落点);per-(namespace, song_value, key) | ~220 行 |
| `src/db/schema.sql`(改) | 追加 `CREATE TABLE IF NOT EXISTS song_kv (...)`;`song_stats` 追加 `rating INTEGER` 列(ALTER 用 `ADD COLUMN`,见 §4 迁移) | +12 行 |
| `src/db/namespace.rs`(改) | `NamespaceStore` 追加 `kv()` 取 KV 视图 + `set_rating`/`query_rating`(rating 是一等字段,落 song_stats) | +60 行 |
| `src/lib.rs`(改) | `pub use` 追加 `SongKvStore` / `SongKvEntry` | +2 行 |

### mineral-tui(增量)

| 文件 | 职责 | 规模 |
|---|---|---|
| `src/runtime/keymap.rs`(sub00 已建,**追加**) | 命中"自定义动作名"分支 → 采集 `KeyContext` → `client.key_triggered(...)`;持有从握手拿到的自定义动作名集合 | +约 80 行 |
| `src/runtime/keyctx.rs`(新) | `collect_key_context(&AppState) -> KeyContext`:从 view/sel_*/current 取只读快照 | ~90 行 |
| `src/runtime/remote.rs`(改) | `RemoteClient` 追加 `key_triggered` / queue 双寻址 / store 命令的 `send_recv`;消费握手回执里的自定义动作名 | +约 70 行 |

### mineral-server(增量)

| 文件 | 职责 | 规模 |
|---|---|---|
| `src/serve.rs`(改) | dispatch 追加 `Request::KeyAction`(投递脚本线程)/ queue 双寻址 / store 命令;握手回执填 `custom_actions`(向脚本线程查注册表名集合) | +约 90 行 |
| `src/client.rs`(改) | `Client` trait 追加 `key_triggered` / queue 双寻址 / store 方法;`ClientHandle` in-proc 实现 | +约 80 行 |

## 4. 关键类型与签名

### 4.1 协议:KeyContext / KeyTriggered(**复用 sub03 `mineral-protocol/src/key.rs`,本 subspec 不重定义**)

**已裁决**(交叉审查):`KeyContext` / `KeyTriggered` / 视图枚举的 canonical 定义归 **sub03**(Phase 1 前置),本 subspec **只消费**,不重定义。统一形状如下(详见 sub03 §4.4):

- `mineral_protocol::KeyTriggered { action: String, ctx: KeyContext }`(字段名是 **`ctx`**,非 `context`)。
- `mineral_protocol::KeyContext`:**私有字段 + `TypedBuilder` + `Getters` + `#[non_exhaustive]`**(对外配置 struct 约定,**非**裸 pub DTO),字段 `view: ViewKind` / `selected_song_id` / `selected_playlist_id` / `now_playing_id`。
- `mineral_protocol::ViewKind`:**5 变体** `Playlists / Tracks / Queue / Fullscreen / Search`(**非** 本文旧版的 `KeyView{Playlists, Library}` 二值枚举)。

本 subspec 的 TUI 侧采集(`keyctx.rs`)用 `KeyContext::builder()` 造实例(经 getter 读),不再以字面量构造裸 DTO;TUI 的 `View{Playlists, Library}` 在采集时映射到 `ViewKind`(`Library` → `ViewKind::Tracks`,全屏/搜索/队列态映射到对应 `ViewKind`)。

> 键字符串 ↔ crossterm `KeyEvent` 的规范化解析在 TUI 侧 `keymap.rs`(`KeyChord` 类型与字符串解析器归 `mineral-config::keys`,见 sub00 §10),协议层不掺。

### 4.2 协议:queue 双寻址与 store(`message.rs` 追加)

> **已裁决**:队列稳定句柄统一用 sub03 的 **`mineral_protocol::EntryId`**(归 `queue.rs`,私有 `u64`),本 subspec **不**定义 `QueueEntryId`,只消费 `EntryId`。entry-id / version 协议字段归 sub03。

```rust
/// `Request` 追加(节选):
/// 自定义动作触发(daemon VM 执行 fn(ctx))。返回 `Response::Ok`。
/// **已裁决变体名 `KeyAction`(沿用 sub03 Phase 1 前置定义),非 `KeyTriggered`**。
KeyAction(crate::KeyTriggered),
/// 跳到队列指定条目(pos 或 entry-id 任一;二者矛盾时 entry-id 优先)。返回 `Response::Ok`。
QueueJump { pos: Option<usize>, entry: Option<crate::EntryId> },
/// 读 per-song 持久值。返回 `Response::StoreValue`。
StoreGet { song: SongId, key: String },
/// 写 per-song 持久值。返回 `Response::Ok`。
StoreSet { song: SongId, key: String, value: StoreValue },
/// per-song 数值自增(`inc`)。返回 `Response::StoreValue`(自增后的值)。
StoreInc { song: SongId, key: String, delta: i64 },

/// store 值域:Lua 可表达且能 bincode 落库的标量(开放 key)/ 一等字段亦走它。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StoreValue {
    /// 整数(`local_play_count` / `rating` 等)。
    Int(i64),
    /// 浮点。
    Real(f64),
    /// 文本。
    Text(String),
    /// 布尔。
    Bool(bool),
    /// 缺失(未设置过该 key)。
    Nil,
}
```

> Lua 侧 song_id 句柄用 `SongId::qualified()`(`namespace:value`)字符串;TUI/CLI 调 store
> 时仍传结构化 `SongId`(主文档"rust 内部一定优先结构化,边缘适配层再序列化")。

### 4.3 persist:开放 KV 表 + rating 一等字段(`db/song_kv.rs`)

```rust
//! per-song 开放命名空间 KV(`store.*` 的开放 key 落点);内建一等字段不走这里,走 song_stats。

/// 一条开放 KV(出参)。
#[derive(Debug, Clone, PartialEq)]
pub struct SongKvEntry {
    /// 键(开放命名空间,约定 `plugin.*` / 用户自取;一等字段名为保留键,拒绝写入)。
    pub key: String,
    /// 值(标量)。
    pub value: mineral_protocol::StoreValue,
}

impl NamespaceStore {
    /// 读一条开放 KV;降级 / 未命中返回 `Ok(StoreValue::Nil)`。
    ///
    /// # Params:
    ///   - `id`: 歌曲 id(其裸值入库,namespace 由本 store 隐含)
    ///   - `key`: 开放键(保留键 `local_play_count`/`rating`/`last_played` 应改走一等字段方法)
    ///
    /// # Return:
    ///   命中返回标量值,未命中返回 `StoreValue::Nil`。
    pub async fn kv_get(&self, id: &SongId, key: &str)
        -> color_eyre::Result<mineral_protocol::StoreValue> { /* ... */ }

    /// 写一条开放 KV(upsert)。降级 no-op。保留键传入返回 `Err`(调用方应走一等字段)。
    pub async fn kv_set(&self, id: &SongId, key: &str, value: &mineral_protocol::StoreValue)
        -> color_eyre::Result<()> { /* ... */ }

    /// 设/取消 rating(一等字段,落 song_stats.rating;`None` 清空)。降级 no-op。
    pub async fn set_rating(&self, id: &SongId, rating: Option<u8>)
        -> color_eyre::Result<()> { /* ... */ }
}
```

**schema 追加**(`schema.sql`):

```sql
CREATE TABLE IF NOT EXISTS song_kv (
    namespace TEXT NOT NULL,
    song_value TEXT NOT NULL,
    key TEXT NOT NULL,
    -- 标量值:类型标签 + 三个候选列(只填其一),读出时按 vtype 重建 StoreValue。
    vtype TEXT NOT NULL,           -- 'int' | 'real' | 'text' | 'bool'
    int_val INTEGER, real_val REAL, text_val TEXT,
    PRIMARY KEY (namespace, song_value, key));
```

> rating 用 `ALTER TABLE song_stats ADD COLUMN rating INTEGER`,但 `ensure_schema` 当前是整段
> `raw_sql`(`schema.rs:17`)幂等 CREATE。**迁移策略**:`ALTER ADD COLUMN` 非幂等(列已存在会报错),
> 故 rating 列加进单独的"尽力 ALTER、忽略 duplicate column 错误"步骤,放 `ensure_schema` 末尾
> 用 `serde_path_to_error` 不适用、改用显式 `match` 吞 `duplicate column name` 一类错误(**不**用
> `.map_err(|_| ...)`,要按 sqlite 错误码判别,见 §7 风险)。

### 4.4 Lua 一等字段与开放 key 命名空间约定

| Lua key | 落点 | 语义 |
|---|---|---|
| `local_play_count` | `song_stats.play_count`(只读映射) | 本地完整播放次数(已由 `record_play` 维护) |
| `rating` | `song_stats.rating`(新列,可写 0..=5) | 用户评分 |
| `last_played` | `song_stats.last_played_at`(只读) | unix ms |
| 其它(如 `plugin.skipcount`) | `song_kv` 表 | 开放命名空间,约定带 `.` 前缀避免与未来一等字段冲突 |

写一等字段以外的保留名 → `Err` toast;写开放 key 无前缀 → warn 但允许(渐进约定)。

### 4.5 timer handle(`mineral-script/src/api/timer.rs`)

```rust
/// 脚本线程内的定时器。由 `timer.after(ms, fn)` / `timer.every(ms, fn)` 创建,
/// 返回给 Lua 一个 userdata,暴露 `stop` / `kill` / `resume`。
///
/// **不另起 tokio**:定时器挂在脚本线程已有的 tick 心跳上(sub04 的看门狗 / 事件 loop),
/// 每拍检查到期。`stop` 保留已走计时(暂停);`kill` 注销并清零;`resume` 从暂停处续。
struct ScriptTimer {
    /// 间隔(`every`)或一次性延迟(`after`)。
    interval: std::time::Duration,
    /// 距下次触发的剩余时间(`stop` 时冻结此值)。
    remaining: std::time::Duration,
    /// `true` = 周期触发(every);`false` = 一次性(after,触发后自动 kill)。
    repeating: bool,
    /// 暂停标志(`stop` 置 true、`resume` 置 false)。
    paused: bool,
    /// 回调在 VM 注册表里的句柄。
    callback: mlua::RegistryKey,
}
```

### 4.6 Client trait 追加(`mineral-server/src/client.rs`)

```rust
/// 自定义动作触发:把键转发给 daemon VM 执行 fn(ctx)。fire-and-forget。
fn key_triggered(&self, payload: mineral_protocol::KeyTriggered);

/// 跳到队列指定条目(pos / entry-id 双寻址)。
fn queue_jump(&self, pos: Option<usize>, entry: Option<mineral_protocol::EntryId>);

/// 读 per-song 持久值;不可用 / 未命中返回 `StoreValue::Nil`。
fn store_get(&self, song: SongId, key: String) -> mineral_protocol::StoreValue;
```

> in-proc(`ClientHandle`)实现:`key_triggered` 经 player 投递脚本线程命令通道;store 经
> `persist().scope(..)`(async,in-proc 用 `tokio::spawn` fire-and-forget,read 返回 Nil 占位,
> 与既有 `query_song_stats` in-proc 降级同范式 — `client.rs:266`)。

### 4.7 Lua↔Song table 编解码契约 + `SearchHits`(**本 subspec owner**)

**已裁决**:`library.tracks` / 搜索类 API 共用的 **Lua table ↔ `mineral_model::Song` 编解码** 与 **`SearchHits` 类型**的定义 owner 归本 subspec(由 **PR-H**(library)一并定义,接线 PR-F/PR-H 复用)。这是给 sub06(Phase 3 `library.search`)的承诺:sub06 的 `run_search` 复用本 subspec 定义的同一份 `SearchHits` 与 Song table 编解码,**不另造**。

- **Song table 编解码**(`mineral-script/src/api/library.rs`):`song_to_lua(&Song, &Lua) -> mlua::Table`(单向投影,字段子集:`id` 用 `qualified()` 字符串、`title`/`artist`/`album`/`duration_ms` 等只读;不暴露内部 dto)与按需的 `lua_to_song_id(&mlua::Value) -> Result<SongId>`(命令入参只需 id,不需整首回传)。`library.tracks()` 与 `library.search`(sub06)走同一投影,保证 Lua 侧 song table 形状一致。
- **`SearchHits`**:结构化命中集合(`Vec<Song>` 按 scope 分类的薄封装),边缘才转 Lua table。sub06 §6 接口锚点的"`SearchHits` / Lua↔Song table 编解码——以 sub05 定义为准"由此满足。

### 4.8 player 级播放失败信号 → `track_finished("error")`(**本 subspec owner,接 sub04 §6 缺口**)

**已裁决**(spec 审计):sub04 §6 把 `Error` reason 推迟到 Phase 2,主 spec §6 同标"`error` 变体随 player 级播放失败信号补齐",二者无 owner——本 subspec **认领**。

现状缺口:`play_song`(`crates/mineral-server/src/player.rs:290`)只向 scheduler 提交 SongUrl 任务,取链 / 解码失败时仅进 `TaskOutcome::Failed`(任务层),**不经 player 状态、也不产任何 `track_finished`**。Phase 1 的 `notify_track_finished` 只接 `Eof`/`Skip`(可靠)+ `Stop`(best-effort),`Error` 分支悬空。

**范围与接缝**(实现细节"writing-plans 阶段细化",此处只钉范围 / 接缝 / 验收):

- **接缝点**:`play_song` 的**失败路径**(SongUrl 任务返 `TaskOutcome::Failed` / 解码无法继续)是唯一信号源。把"该首播放无法继续"从任务层提升为一条 player 级信号(具体形态——同步返回 `Err` 后由 `play_song` 调用者补投、还是 player 内部状态机增一个失败态——留 writing-plans 拍),由 `PlayerCore` 调既有钩子 `notify_track_finished(failed_song, TrackFinishedReason::Error)`(sub04 §6 落地做法的第四个判定点),投出 `ScriptEvent::TrackFinished { song, reason: Error }`。
- **不改散落判定**:复用 sub04 §6 已定的 `notify_track_finished` 钩子与四判定点框架,本项只补齐其中 `Error` 这一路的信号来源,**不新设独立事件类型**。
- **`song` 取值**:失败前 `play_song` 入参的目标 Song(即"想播但没播成的那首"),与 `Eof`/`Skip` 的"刚结束的那首"语义一致。
- **降级**:daemon 未持 VM / 无脚本线程时,`notify_track_finished` 是 no-op(对齐 sub04),失败路径其余行为(UI 报错 toast 等)不变。

**验收判据**(进 §6.5 daemon-e2e 串行组):

- `track_finished_error_on_url_failure`:`MINERAL_AUDIO_NULL=1` 起 daemon,配置注册 `mineral.on("track_finished", fn)`(fn 用 `store.inc` 或 toast 记 reason),**注入一个 SongUrl 解析必失败的歌**(mock channel 提供"取链恒失败"的 song,或 netease 无效 id),`player.play(song_id)` → 断言 Lua hook 收到 `reason == "error"` 的 `track_finished`(对齐 sub04 §8 的 `on("track_finished")` e2e 范式)。

## 5. 实现步骤(依赖顺序,可拆 PR)

> 前置:sub00/sub01/sub03/sub04 已落 Phase 0/1(sub00 `Action` 枚举统一 + 声明式 keymap、
> sub01 `mineral-config` loader、sub03 Event 推送 + request-id 配对 + 握手骨架、
> sub04 `mineral-script` 骨架与脚本线程 + 四承重墙最小集)。每步独立 PR、自带测试。

1. **PR-A persist 底座**:`song_kv` 表 + `rating` 列迁移 + `SongKvStore` / `kv_get`/`kv_set`/
   `set_rating`/`query_rating`。纯 mineral-persist,无 Lua 依赖,先行。测试见 §6.1。
2. **PR-B 协议类型**:**复用 sub03 的 `key.rs`(KeyContext/KeyTriggered/ViewKind)与 `EntryId`,不重定义**;本 PR 仅新增 `StoreValue`、`Request`/`Response` 追加 variant(`KeyAction`/`QueueJump`/`Store*`)、追加 `Event::StoreChanged`(经 sub03 owner 文件 event.rs)、握手回执 `custom_actions` 字段。纯 mineral-protocol。codec round-trip 测试。
3. **PR-C Client + serve 接线**:`Client` trait 追加方法 + `ClientHandle` in-proc 实现 +
   serve dispatch 追加分支 + 握手回执填 `custom_actions`(向脚本线程查 `ActionRegistry`)。
4. **PR-D store API(Lua)**:`mineral-script/src/api/store.rs` + `registry.rs` 的 store dispatch;
   一等字段路由 + 开放 key;写后投递 **`Event::StoreChanged { song_id, key }`**(独立粗粒度事件,经 sub01/sub04 的 push 通道)。
5. **PR-E action/bind**:`api/action.rs` + `ActionRegistry`;`bind` 语法糖追加声明切片 keys;
   握手下发自定义动作名集合(接 PR-C 的回执)。
6. **PR-F TUI 键转发**:`keyctx.rs` 采集 + `keymap.rs` 命中自定义动作名分支 → `key_triggered`;
   `RemoteClient` 接线。e2e:绑键 → 按键 → daemon VM 执行可见副作用(toast / store)。
7. **PR-G queue 双寻址**:`api/queue.rs` + 协议 entry-id 编织 + player queue 加 entry-id 分配
   与单调 version(若 sub03 未加 version,本 PR 补)。
8. **PR-H library API**:`api/library.rs` 映射 `my_playlists`/`songs_in_playlist`/`liked_song_ids`/
   `set_loved`(经 player 命令通道,async 结果回投脚本线程)。
9. **PR-I timer**:`api/timer.rs` + `ScriptTimer` 挂脚本线程 tick;`stop`/`kill`/`resume`。
10. **PR-J 热重载**:`reload.rs` notify watch + debounce → 重 eval 新 VM → `ActionRegistry`
    原子换 → 声明切片 diff → push;TUI 收 diff 覆盖 keymap / theme。
11. **PR-K player 级播放失败信号 + `track_finished("error")`**(§4.8,可独立):mineral-server 在
    `play_song` 失败路径把 SongUrl 解析 / 解码失败从 `TaskOutcome::Failed` 提升出 player 级信号,
    调 sub04 §6 的 `notify_track_finished(song, Error)` 投 `ScriptEvent::TrackFinished{ reason: Error }`。
    纯 server 失败路径接线,不碰 Lua API 表面。e2e 见 §4.8 / §6.5。
12. **PR-L cleanup:退役 `TaskEvent::Notice` 旧通道**(收尾,排在热重载 PR-J 之后):Phase 2 末
    ——toast 全量走 `Event::Toast`(下载完成等提示均经 daemon VM 产 `Event` 通道,sub03 已建)
    ——后,移除 `TaskEvent::Notice`(`crates/mineral-task/src/event.rs:67-72`)及 client 侧
    `DrainTaskEvents` → `notifications.flash_text` 拉取路径(`app.rs:384-395`)。引用 **sub03 §7.6 /
    §8 裁决**:`TaskEvent::Notice` **保留至 daemon VM(sub04)产 `Event` 后再退役**,本 PR 兑现该退役
    时点。验收:下载完成 toast 仍可见(改走 `Event::Toast`),全仓无 `TaskEvent::Notice` 引用残留。

PR-A/B/C 可并行(无相互依赖);D 依赖 A+C;E 依赖 C;F 依赖 E;G 依赖 C(+queue version);
H 依赖 C;I 依赖脚本线程(sub04);J 依赖 E(注册表换)+ sub03/sub04 推送通道;
K 依赖 sub04 的 `notify_track_finished` 钩子与 `ScriptEvent::TrackFinished`(其余独立,可与 D–J 并行);
L(收尾)依赖 toast 全量走 `Event::Toast`(下载提示已脱离 `TaskEvent::Notice`),排在 J 之后。

## 6. 测试清单(对齐 docs/testing.md)

### 6.1 persist(PR-A,`#[tokio::test]`,sqlite tempdir)

- `song_kv_roundtrip`:`kv_set` 各 `StoreValue` 变体 → `kv_get` 还原(含 `Nil` 未命中)。
- `song_kv_isolated_by_source`:两 namespace 同裸值 song 同 key 互不串(对齐既有
  `loved_ids_isolated_by_source` — `namespace.rs:788`)。
- `set_rating_then_query` + `rating_clear_with_none`。
- `kv_set_reserved_key_errors`:写 `local_play_count` / `rating` 返回 `Err`。
- `ensure_schema_idempotent_with_rating_column`:`ensure_schema` 跑两次不因 `ADD COLUMN`
  报错(扩展既有 `ensure_schema_is_idempotent` — `schema.rs:33`)。

### 6.2 协议(PR-B,`tests/codec.rs` 扩展)

- `KeyTriggered` / `StoreValue` 各变体 / `QueueJump` 双寻址 bincode encode→decode 等价
  (对齐既有 `crates/mineral-protocol/tests/codec.rs`,`assert_eq!`)。

### 6.3 store / action Lua(PR-D/E,`mineral-script` 单测;`multi_thread` rt)

- `store_set_get_inc`:Lua `store.set(id,'plugin.x',1); store.inc(id,'plugin.x',2)` →
  `store.get` 得 3(走真实 sqlite tempdir + 脚本线程)。
- `store_first_class_rating`:Lua `store.set(id,'rating',4)` → persist `query_rating` 得 4。
- `action_register_then_invoke`:`mineral.action('my.x', fn)` 注册 → 经命令通道触发 → fn 跑
  (用 `store.inc` 计数验证副作用,避免 toast 排版断言)。
- `bind_appends_keymap_slice`:`mineral.bind('S', fn)` 后,声明切片 keys 含 `S → <匿名 action>`。
- **教训**:起脚本线程 + 真实 persist 的 test 必须 `#[tokio::test(flavor = "multi_thread")]`
  (MEMORY:真实 IO/engine 测试需 multi_thread,否则全仓并发 flaky)。

### 6.4 TUI 键转发(PR-F,`test_support::app_with_*` + 真实 KeyEvent)

- `custom_key_collects_context_and_forwards`:用 `TestClient` 记录 `key_triggered` 调用;
  `app_with_library` 选中第 0 首 → 按绑定到自定义动作的键 → 经 getter 断言
  `ctx.selected_song_id()` == 该首 id、`ctx.view() == ViewKind::Tracks`(Library 视图映射)、
  `ctx.now_playing_id()` 与 `state.current` 一致(`KeyContext` 私有字段,经 sub03 getter 读)。跨 tick(对齐
  `app.rs:773` 的 `press` helper + `queue_nav_moves_and_survives_snapshot_tick` 范式)。
- `keyctx_playlists_view`:Playlists 视图选中歌单 → `ctx.selected_playlist_id()` 命中、
  `ctx.selected_song_id() == None`、`ctx.view() == ViewKind::Playlists`。

### 6.5 queue / library / timer / 播放失败 e2e(PR-G/H/I/K,daemon-e2e 串行组)

- daemon 子进程 e2e 走 `.config/nextest.toml` 的 `daemon-e2e` 组(`binary(daemon_lifecycle)`
  过滤)+ `MINERAL_AUDIO_NULL=1`:Lua `queue.jump{ entry = ... }` → snapshot 反映;
  `library.playlists()` 返回非空(mock channel);`timer.after(0, fn)` 一拍后 fn 跑。
- queue version 单调:连续两次结构变更后 `QueueSnapshot.version` 严格递增(单测,不必 e2e)。
- `track_finished_error_on_url_failure`(PR-K):同组 daemon e2e,注册
  `mineral.on("track_finished", fn)` + 注入 SongUrl 取链恒失败的歌 → `player.play(song_id)` →
  断言 Lua hook 收到 `reason == "error"`(详见 §4.8)。

### 6.6 热重载(PR-J)

- `reload_swaps_action_registry`:写 config.lua v1(action `a`)→ eval → watch 触发改成
  action `b` → 重 eval → 注册表只含 `b`(原子换,无中间态双注册)。用临时文件 + 直接调
  reload 入口(不必真等 notify debounce;notify 本身的触达另有一条 `#[ignore]` 慢测)。
- `reload_pushes_keymap_diff`:重载后声明切片 keys 变化经 push 通道下发(断言 Event)。
- 快照:`mineral config check` 在含自定义 action 的 config 下的输出走 `assert_snap!` 带中文
  `description`(对齐主文档 §14;`INSTA_UPDATE=always` 严禁)。

## 7. 验收判据与风险

### 验收判据(对齐主文档 §15 Phase 2 交付判据)

1. `mineral.action('my.x', fn) + mineral.bind('S', 'my.x')`(或匿名 `bind('S', fn)`)后,
   TUI 按 `S` 能触发 daemon VM 执行 `fn(ctx)`,`ctx` 字段与按键瞬间选中/在播一致。
2. `store.set/get/inc` 持久可见:重启 daemon 后 `store.get` 仍读到上次写入;一等字段
   `local_play_count` 随播放自增、`rating` 可写读。
3. `queue.list` 返回 pos + entry-id;`queue.jump{ entry = id }` 在重排后仍跳对条目。
4. `library.playlists/tracks/love` 经 channel 返回真实数据(登录态)/缓存(离线)。
5. `timer.after/every` 触发回调;`stop` 暂停保留计时、`resume` 续、`kill` 注销。
6. 热重载改 config.lua 后,新 action 生效、旧 action 失效、keymap/theme 切片在 TUI 实时覆盖,
   **无需重启**;重载失败落 toast + 保留旧 VM(不空窗)。
7. 不写配置 / 不用这些 API 时,行为与今天完全一致(降级 persist / 无脚本线程时全 no-op)。

### 风险与缓解

| 风险 | 缓解 |
|---|---|
| `ALTER TABLE ADD COLUMN rating` 非幂等(列已存在报错),整段 `raw_sql` 容不下 | 单独步骤执行 ALTER,按 sqlite 错误码判别 `duplicate column name` 吞掉(**不**用 `map_err(\|_\|..)` 丢上下文,违反 lint);其余错误冒泡 |
| **KeyContext/KeyTriggered 形状**:sub03(Phase 1)与本文旧 §4.1 形状曾不一致 | **已裁决**:统一**消费 sub03 形状**——`KeyContext`(私有字段 + TypedBuilder + Getters + `#[non_exhaustive]`,`view: ViewKind`{Playlists/Tracks/Queue/Fullscreen/Search})、`KeyTriggered{action, ctx}`(字段名 `ctx`)。本 subspec 不重定义 key.rs,只 `use`(§4.1) |
| **queue 句柄类型名**:sub03 `EntryId` vs 本文旧 `QueueEntryId` | **已裁决**:统一用 sub03 的 **`EntryId`**(`queue.rs`,私有 `u64`);entry-id/version 协议字段归 sub03,本 subspec 仅消费(§4.2) |
| **`Request` 转发变体名**:sub03 `Request::KeyAction` vs 本文旧 `Request::KeyTriggered` | **已裁决**:统一用 **`Request::KeyAction(KeyTriggered)`**(沿用 sub03 Phase 1 前置定义)(§4.2) |
| store 写变更走 `PropertyChanged` 还是独立事件 | **已裁决**:走**独立 `Event::StoreChanged { song_id, key }`**(粗粒度,MPD sticker 子系统风格),**不**复用 `PropertyChanged`——observe 的"订阅即回放 + 末值合并"语义只对有限属性树成立,per-song KV 是开放命名空间的数据库不是属性树(主 spec §8 范围注)。该变体归 sub03 event.rs owner,本 subspec PR 追加 |
| `key_triggered` 是 fire-and-forget,但 store 读(`store_get`)要同步返回值,in-proc 无法 block-on-async | in-proc read 返回 `Nil` 占位(对齐 `query_song_stats` in-proc 降级);daemon 模式经 IPC 拿真值;Lua 文档注明 in-proc 调试模式 store 读不可用 |
| 慢 timer 回调卡脚本线程 | 复用 sub04 看门狗(指令计数 + 墙钟双阈值);timer 回调与 hook 同熔断 |
| `KeyContext` 字段未来膨胀(需更多视图态判别值) | wire DTO 加字段是兼容增量(bincode 顺序敏感,**追加在末尾**;`#[serde(default)]` 兜旧 client) |
| 单文件逼近 800 行(`message.rs` / `api/store.rs`) | 协议 store 类型若超量拆 `src/store_wire.rs`;Lua store 绑定与 dispatch 分文件 |
