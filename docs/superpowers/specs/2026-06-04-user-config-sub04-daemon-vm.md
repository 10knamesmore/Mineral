# Subspec 04 · mineral-script:daemon VM 运行时(Phase 1)

> 父文档:[`2026-06-04-user-config-lua-design.md`](./2026-06-04-user-config-lua-design.md)(已定宪法,本文不重辩,只落地)。
> 对应分期:父文档 §15 **Phase 1**。本文档按项目约定不纳入版本控制。
> 状态:实现级 subspec,open questions 已裁决(接缝接法 B / track_finished reason 分期 / download path / 看门狗转换 / ScriptCmd drain,见 §3/§5/§6/§10),可进实现计划。

## 1. 范围与不做

**本 subspec 负责**(父文档 §6 / §11「mineral-script(新)」/ §13 看门狗):

- 新 crate **`mineral-script`**:daemon 进程内**唯一常驻 Lua VM** 的运行时。
- VM 线程所有权模型:`mlua::Lua` 是 `Send + !Sync`,VM + 注册表归一条**专用脚本线程**;daemon 主循环只经 channel 投递,绝不直接持 `Lua`。
- 四承重墙 host API 绑定(Phase 1 最小集,父文档 §6 / §9 P1):
  - ① 结构化命令 `mineral.player.*` / `mineral.download`
  - ② 属性树 `mineral.get` / `mineral.observe`(订阅即回放 + 末值合并两条语义)
  - ③ 离散事件 `mineral.on("track_finished"/"download_completed", fn)`(必带 reason / path)
  - ④ 具名动作 `mineral.action(name, fn)`(**仅注册表**,键转发与 `KeyContext` 是 sub05 / Phase 2,本期不接物理键)
- 工具绑定:`mineral.ui.toast` → push 通道(`Event::Toast`);`mineral.log.info/warn`。
- 看门狗:`mlua::Lua::set_hook` 指令计数 + 墙钟双阈值,中断慢/死循环 hook。
- 与 `mineral-server` 的接缝:谁 spawn 脚本线程、领域事件怎么喂进来。

**明确不做**(交给相邻 subspec / 后续 Phase):

- **不做** loader / merge / `default.lua` / 强类型 `Config` —— 属 **sub01**(`mineral-config`)。本期只**消费** sub01 产出的「daemon 配置切片」与「已 eval 的 hook 注册闭包」,见 §3 接缝。
- **不定义** `Event` / `KeyContext` / `KeyTriggered` / 握手协议的 wire 形状 —— 属 **sub03**(`mineral-protocol`)。本期只**生产** `Event` 实例往 push 通道塞,字段名以 sub03 为准(§9 列出依赖的字段)。
- **不做** `mineral.bind` / `KeyTriggered` 闭环、`queue/library/store/timer` API、热重载 —— 父文档 §9 P2 / §15 Phase 2。
- **不做** `before_play/before_download` 同步拦截、`emit/on_message`、`spawn`、`search` —— P3。
- **不改** TUI 消费侧(`Theme::from_config`、keymap、Event 消费)—— 属 sub02 / sub05。
- **不做安全沙箱**(父文档 D10):`io/os/require` 全开,唯一防线是看门狗。

边界一句话:**sub01 给我「配置 + 已注册的 Lua 闭包」,sub03 给我「Event/协议形状」,我把领域事件喂给闭包、把闭包的副作用变成 Event 推回去,并在专线程上用看门狗护住 daemon 主循环。**

## 2. 现状锚点(file:line,真实代码)

事件源(领域态在哪产生 / 变化):

- 曲终判定(EOF reason 的唯一来源):`mineral-server/src/gapless.rs:254` `check_advance` —— `snap.track_finished_seq` 前进即曲终,`spawn_on_played(old, /*completed*/ true, listen_ms)`(`gapless.rs:282`)。自然播完 = `completed=true`。
- 手动切歌(skip reason):`mineral-server/src/player.rs:478` `next_song`、`player.rs:451` `prev_or_restart` —— 都 `spawn_on_played(old, /*completed*/ false, …)`(`player.rs:489` / `player.rs:471`)。
- 起播(新曲身份):`mineral-server/src/player.rs:290` `play_song`;`current_track_token` 每次起播 / 边界轮转 +1(`mineral-audio/src/snapshot.rs:52`)。
- auto-next 主循环:`mineral-server/src/player.rs:557` `background_loop`(每 `TICK_INTERVAL_MS=20ms`,`player.rs:41`),依次 `consume_events_once` / `check_harvest` / `check_advance` / `check_prefetch` / `check_session_save`。
- 状态快照源(observe 的取值面):
  - 音频态:`mineral-audio/src/snapshot.rs:18` `AudioSnapshot`(`playing` / `position_ms` / `duration_ms` / `volume_pct` / `track_finished_seq` / `current_track_token` / `backend`)。
  - 业务态:`mineral-protocol/src/player.rs:131` `PlayerSnapshot`(`current_song` / `play_mode` / `queue` / `queue_sel`)。
  - 取法:`PlayerCore::snapshot()`(`player.rs:145`)+ `PlayerCore::audio_snapshot()`(`player.rs:270`)。
- 下载完成事件源:`mineral-server/src/download.rs:345` `finalize`(`result_seq +=1`),逐首结果 `last_ok/last_skip/last_fail`(`download.rs:329`);单首打点经 `push_notice`(`player.rs:208` → `TaskEvent::Notice`)。**注**:Phase 1 的 `download_completed(song, path)` 需要**单首**粒度,现状只有批级 `result_seq`,见 §6 / open question。
- push 通道现状(要泛化的雏形):`PlayerCore::push_notice`(`player.rs:208`)往 `client_events: Mutex<Vec<TaskEvent>>`(`player.rs:94`)塞,client 经 `drain_client_events`(`player.rs:157`)拉走;协议侧无主动推送(`mineral-protocol/src/lib.rs:10`「当前不支持异步推送」—— sub03 要建)。

「轮询快照 + 末值 diff」的现成范式参照(observe 实现的镜子):`mineral-server/src/media.rs:158` `report_loop` —— 200ms tick 拉 `snapshot()` + `audio().snapshot()`,与 `last_song_id` / `last_play_mode` / `last_pos` 比对,只在变化时上报(MPRIS),正是 observe 的「末值合并」语义。

daemon 接缝:

- daemon 入口:`mineral-cli/src/subcommands/serve.rs:26` `run`,`serve.rs:52` `Server::spawn`,`serve.rs:55` `start_media_service`,`serve.rs:59` `server.serve(listener)`。
- Server 容器:`mineral-server/src/server.rs:42` `Server::spawn`(组装 audio + scheduler + PlayerCore + pcm + heartbeat),`server.rs:73` `start_media_service`。
- PlayerCore 长跑 loop spawn:`mineral-server/src/player.rs:136` `tokio::spawn(bg.background_loop())`。

工程基线:

- `mlua` **尚未**进 workspace(`grep mlua Cargo.toml` 空)。父文档 D1:`lua54 + vendored`。需新增 `[workspace.dependencies] mlua`。
- 配置常量种子:`mineral-config/src/lib.rs:7,10`(两个 `pub const`,sub01 退役)。
- e2e 基建:`mineral/tests/daemon_lifecycle.rs`(`CARGO_BIN_EXE_mineral` 起真子进程 + `MINERAL_AUDIO_NULL`),nextest 串行组 `daemon-e2e`(`.config/nextest.toml`,`filter = binary(daemon_lifecycle)`)。
- 错误链:`mineral-log/src/lib.rs:45` `chain`。

## 3. 与 sub01 / sub03 的接缝(依赖契约)

本 crate 不自己 eval 配置文件。**sub01 的 loader 在 daemon 进程里 eval 后,把活 VM 连同已注册的 hook 闭包整体交给本 crate**。两种接法,**已裁决采用接法 B**(理由见 §7)。具体接缝:**daemon 路径** = 建 VM → `mineral_script::install_api`(挂 `mineral` 活表)→ sub01 eval 配置(顶层 `mineral.on/action` 就地注册进 `ScriptHost`)→ `ScriptRuntime::spawn` 移交脚本线程;**非 daemon 进程**(TUI/CLI/守卫测试)走 sub01 的 `inject_noop_host`(no-op stub),不建活 VM、不接本 crate。

- **接法 A(否决)**:sub01 eval 完丢 VM,只给强类型 `Config`;本 crate 重新 eval 一遍拿 hooks。→ 顶层副作用执行两次,且 `mineral.on/action` 注册无处落。
- **接法 B(采用)**:本 crate 提供注册表对象(`ScriptHost`),sub01 在 eval **前**把 `mineral` 全局表(含 `on/observe/action/player/ui/log/get/download`)注入 VM;eval 配置时这些绑定就地把闭包写进注册表。eval 完整个 VM(含注册表)移交脚本线程。

因此跨 crate 形状:

- sub01 → 本 crate:`fn install_api(lua: &mlua::Lua, host: &ScriptHost) -> mlua::Result<()>`(本 crate 导出;sub01 在建 VM 时调一次,把 `mineral` 表挂上去)。`ScriptHost` 持各注册表的 `Arc`,与脚本线程共享。
- 本 crate → sub01:消费 sub01 的 `DaemonConfig` 切片(看门狗阈值字段,父文档 §13「阈值进 daemon 配置段」),getter 读取。
- 本 crate → sub03:生产 `mineral_protocol::Event`(`Toast` / `PropertyChanged`)塞进 push sink。**依赖 sub03 定义**:`Event` enum、`ToastKind`、`PropName`、`PropValue`(observe 的属性键与值载体)。本期对这些只读字段名,见 §9。

## 4. 新增 / 修改文件清单

新增 crate `crates/mineral-script/`(守单文件 ≤800 行;`lib.rs` 只导出不写逻辑):

| 文件 | 职责 | 预估行 |
|---|---|---|
| `Cargo.toml` | 依赖 `mlua = { workspace = true }`(canonical 条目见 sub01,含 `send`)、`mineral-config`、`mineral-protocol`、`mineral-model`、`mineral-log`、`color-eyre`、`tokio`(sync)、`parking_lot` | — |
| `src/lib.rs` | 模块组织 + `pub use`(`ScriptHost` / `ScriptRuntime` / `install_api` / `ScriptEvent` / `WatchdogConfig`);**不写逻辑** | ~30 |
| `src/host.rs` | `ScriptHost`:注册表聚合体(Arc 共享给脚本线程),`install_api` 把 `mineral` 表挂进 `Lua`。各子 API 模块的装配入口 | ~120 |
| `src/runtime.rs` | `ScriptRuntime`:**专用脚本线程**的所有权容器。`spawn(host, cfg)` 起线程、持 `Lua`、跑消息循环;对外只发 `ScriptSender`(channel 投递端)。drop 优雅停线程 | ~180 |
| `src/message.rs` | 脚本线程的输入消息类型 `ScriptEvent`(daemon→VM)+ 内部 `ScriptCmd`;`ScriptSender` newtype 包 channel | ~120 |
| `src/api/mod.rs` | api 子模块组织 + 各 `install_*` 汇总到 `host.rs`;**不写逻辑** | ~25 |
| `src/api/player.rs` | ① 命令族绑定:`mineral.player.{toggle,next,prev,stop,seek_rel,seek_to,set_volume,set_mode,play}` / `mineral.download` → 经 `ScriptSender` 发 `ScriptCmd::Player(..)` 给 daemon 侧执行 | ~180 |
| `src/api/observe.rs` | ② `mineral.get` 同步读末值缓存;`mineral.observe(prop, fn)` 注册订阅 + 立刻回放当前值。订阅表 + 合并策略 + 节流 | ~220 |
| `src/api/events.rs` | ③ `mineral.on(event, fn)`:`track_finished` / `download_completed` 注册表;reason 枚举与分发 | ~140 |
| `src/api/action.rs` | ④ `mineral.action(name, fn)`:具名动作注册表(Phase 1 仅注册,不接键);名集合 getter 供未来 attach 握手下发 | ~80 |
| `src/api/ui.rs` | `mineral.ui.toast(msg, opts)` → `Event::Toast` 入 push sink;`mineral.log.{info,warn}` → `mineral_log` | ~110 |
| `src/watchdog.rs` | `WatchdogConfig`(对外配置 struct:私有字段 + `#[non_exhaustive]` + getters;**默认值**,真实读取走 sub01 切片)+ `install_hook`(`set_hook` 指令计数 + 墙钟双阈值 + 中断) | ~160 |
| `src/dispatch.rs` | 脚本线程消息循环核心:`ScriptEvent` → 调对应 Lua 闭包(pcall 包裹)→ 收集副作用;observe 末值更新 + 触发回调 | ~220 |
| `tests/` | 单测随各模块 `#[cfg(test)] mod`;e2e 进 `mineral/tests/script_hooks.rs`(见 §6) | — |

修改:

| 文件 | 改动 |
|---|---|
| `Cargo.toml`(workspace) | `members` 加 `"crates/mineral-script"`;`[workspace.dependencies]` 加 `mineral-script`。**`mlua` workspace 条目的 canonical 声明在 sub01**(features 并集 `["lua54","vendored","serialize","send"]`,含本 crate 跨线程移交所需的 `send`;版本落地时取最新 stable)——本 subspec **不自行钉版本/features**,`mineral-script/Cargo.toml` 引用 `mlua = { workspace = true }`,若 sub04 先于 sub01 落地则由本 PR 临时建该 workspace 条目、sub01 落地后归一为同一条 |
| `crates/mineral-server/Cargo.toml` | 依赖 `mineral-script` |
| `mineral-server/src/server.rs:42` `Server::spawn` | 增 `script: Option<ScriptRuntime>` 字段;spawn 时若 sub01 给了 VM 就持有;`background_loop` 旁挂事件喂入(见 §5 步骤 4) |
| `mineral-server/src/lib.rs` | `pub use` 透出注入入口(`Server::spawn` 多一个 `ScriptRuntime` 参数,或 builder) |
| `mineral-cli/src/subcommands/serve.rs:52` | daemon 起 VM:sub01 eval 配置 → 拿 `ScriptRuntime` → 传给 `Server::spawn` |

> 行数预算:api/* 与 dispatch.rs / observe.rs 是热点,均预留 < 800;若 observe.rs 逼近,把「合并 + 节流策略」抽到 `api/observe_throttle.rs`。

## 5. 关键类型与签名(遵项目约定)

### 5.1 脚本线程所有权模型

`mlua::Lua` 是 `Send + !Sync`(开 `send` feature)。模型:**VM 归一条 `std::thread`,daemon 主循环只发消息**。

```rust
/// daemon → 脚本线程的输入事件。daemon 侧产生领域事件后投递,绝不直接触碰 `Lua`。
///
/// 结构化优先(CLAUDE.md「rust 内部一定优先结构化」):字段是强类型,不在此处序列化。
#[derive(Debug)]
#[non_exhaustive]
pub enum ScriptEvent {
    /// 一首歌结束。`reason` 由 daemon 侧判定(见 §6),`song` 是结束的那首。
    TrackFinished {
        /// 结束的歌曲身份。
        song: mineral_model::Song,
        /// 结束原因。
        reason: TrackFinishedReason,
    },

    /// 一首歌下载完成(永久导出落盘)。
    DownloadCompleted {
        /// 下载完成的歌曲。
        song: mineral_model::Song,
        /// 落盘绝对路径。
        path: std::path::PathBuf,
    },

    /// 属性变更(observe 的喂入面)。daemon 每 tick diff 后把变化的属性投进来。
    PropertyChanged {
        /// 属性键(见 §5.4 `PropKey`)。
        key: PropKey,
        /// 新值。
        value: PropValue,
    },
}

/// `track_finished` 的原因。父文档 §6:`"eof"|"skip"|"error"|"stop"`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackFinishedReason {
    /// 自然播完(`check_advance` 判定 finished_seq 前进)。
    Eof,
    /// 用户切歌(next / prev)。
    Skip,
    /// 解码 / 取链失败导致中断。
    Error,
    /// 显式 stop。
    Stop,
}
```

`ScriptRuntime` 拥有线程句柄与投递端:

```rust
/// daemon 侧持有的脚本运行时句柄。内部一条专用线程独占 `Lua`;本类型只暴露投递端,
/// 调用方(daemon 主循环)拿不到 `Lua` —— 杜绝跨线程持有 `!Sync` 的 VM。
///
/// Drop 时给线程发停止信号并 join(看门狗中断后线程不会卡)。
pub struct ScriptRuntime {
    /// 向脚本线程投递事件 / 命令回执的发送端(clone 廉价)。
    sender: ScriptSender,

    /// 脚本线程 join 句柄(`Drop` 时收尾)。
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ScriptRuntime {
    /// 起脚本线程并接管 `Lua`(连同已注册的 hook 闭包)。
    ///
    /// # Params:
    ///   - `lua`: sub01 已 eval 完、`install_api` 已挂好 `mineral` 表的 VM。
    ///   - `host`: 与 VM 共享的注册表聚合体(脚本线程内调闭包要用)。
    ///   - `watchdog`: 看门狗阈值(来自 sub01 daemon 配置切片)。
    ///
    /// # Return:
    ///   运行时句柄;线程内 `Lua` 移动失败等致命错经 `Err` 返回(daemon 据 D9 降级:不起 VM)。
    pub fn spawn(
        lua: mlua::Lua,
        host: ScriptHost,
        watchdog: WatchdogConfig,
    ) -> color_eyre::Result<Self> { /* ... */ }

    /// 投递一个领域事件给脚本线程(fire-and-forget,不阻塞 daemon 主循环)。
    pub fn send(&self, event: ScriptEvent) {
        self.sender.send(event);
    }

    /// 拿一个廉价 clone 的投递端,交给 daemon 各事件源。
    pub fn sender(&self) -> ScriptSender {
        self.sender.clone()
    }
}
```

channel 选型:`tokio::sync::mpsc::UnboundedSender<ScriptMsg>`(daemon 主循环在 tokio runtime,脚本线程是裸 `std::thread`,跨界用 `mpsc` + 线程内 `blocking_recv` 兜)。`ScriptMsg` 内部 = `ScriptEvent`(daemon→VM)∪ 停止信号。push sink 反向(VM→daemon)用另一条 `UnboundedSender<mineral_protocol::Event>`(sub03 形状),`ScriptHost` 持其 clone。

### 5.2 ScriptHost(注册表聚合 + API 装配)

```rust
/// VM 内 `mineral` 表背后的注册表聚合体。脚本线程独占其消费侧;`install_api` 在装配期
/// 把各注册表的 `Arc` 闭进 Lua 绑定函数,故 `Clone` 廉价(全 Arc)。
#[derive(Clone)]
pub struct ScriptHost {
    /// `mineral.on(...)` 注册的离散事件回调表(按 reason 分类的 `track_finished` 等)。
    events: std::sync::Arc<parking_lot::Mutex<EventRegistry>>,

    /// `mineral.observe(...)` 订阅表 + 末值缓存。
    observers: std::sync::Arc<parking_lot::Mutex<ObserveRegistry>>,

    /// `mineral.action(...)` 具名动作表(Phase 1 仅注册)。
    actions: std::sync::Arc<parking_lot::Mutex<ActionRegistry>>,

    /// 命令出口:`mineral.player.*` 发到 daemon 执行。
    commands: tokio::sync::mpsc::UnboundedSender<ScriptCmd>,

    /// push 出口:`ui.toast` / observe 推回 client(sub03 `Event` 形状)。
    push: tokio::sync::mpsc::UnboundedSender<mineral_protocol::Event>,
}

/// 把 `mineral` 全局表(含 player/get/observe/on/action/ui/log)挂进 `lua`。
///
/// sub01 在建 VM、eval 用户配置**之前**调一次,使配置顶层的 `mineral.on(...)` 等就地注册。
///
/// # Params:
///   - `lua`: 目标 VM。
///   - `host`: 注册表聚合体(各绑定函数闭进其 `Arc`)。
///
/// # Errors:
///   `mlua` 设全局表 / 建函数失败。
pub fn install_api(lua: &mlua::Lua, host: &ScriptHost) -> mlua::Result<()> { /* ... */ }
```

### 5.3 命令族(① 结构化命令)

`mineral.player.*` 是既有 IPC 命令的 Lua 投影,经 channel 回 daemon 执行(不在脚本线程直接调 `PlayerCore`——`PlayerCore` 句柄 `Send+Sync`,但命令应回主 runtime 串行,避免脚本线程并发改播放态)。

```rust
/// 脚本侧发起的领域命令。daemon 主循环 drain 后调对应 `PlayerCore` / `ClientHandle` 方法。
#[derive(Debug)]
#[non_exhaustive]
pub enum ScriptCmd {
    /// `mineral.player.toggle()`:暂停 / 恢复二态切换。
    Toggle,
    /// `next()` / `prev()` / `stop()`。
    Next,
    Prev,
    Stop,
    /// `seek_rel(secs)`:相对当前位置(可负)。
    SeekRel(f64),
    /// `seek_to(secs)`:绝对位置。
    SeekTo(f64),
    /// `set_volume(pct)`:0..=100,越界由 daemon 侧 clamp。
    SetVolume(u8),
    /// `set_mode(mode)`:字符串解析为 `PlayMode`,失败 → toast + 忽略。
    SetMode(mineral_protocol::PlayMode),
    /// `play(song_id)`:按 id 播(daemon 侧从队列 / 详情解出 `Song`)。
    Play(mineral_model::SongId),
    /// `mineral.download(song_id)`。
    Download(mineral_model::SongId),
}
```

> `toggle` 在 daemon 侧落到 `audio().snapshot().playing ? pause() : resume()`(照抄 `media.rs:53`)。`seek_rel/seek_to` 单位是秒(Lua 友好),daemon 侧 `* 1000` 转 ms 喂 `audio().seek`。

> **`ScriptCmd` 的 drain 点(daemon 主循环在哪一拍消费命令通道)已裁决:实现期实测定**。判据:**脚本发起的 player 命令延迟不劣于现有 tick**(`background_loop` 的 `TICK_INTERVAL_MS=20ms`)——若并入 `background_loop` 的某一拍 drain 即可满足判据则就近挂,否则单起一个 select 分支;以实测延迟为准,不先验定方案。

### 5.4 属性树(② get / observe)

```rust
/// 可被 `mineral.get` / `mineral.observe` 寻址的属性键。父文档 §6 列出 P1 子集。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PropKey {
    /// `"player.song"` —— 当前歌(id + 标题等,转成 Lua table)。
    PlayerSong,
    /// `"player.state"` —— "playing"|"paused"|"stopped"。
    PlayerState,
    /// `"player.volume"` —— 0..=100。
    PlayerVolume,
    /// `"player.position"` —— 当前位置秒(高频,节流)。
    PlayerPosition,
    /// `"player.mode"` —— PlayMode label。
    PlayerMode,
    /// `"queue.length"`。
    QueueLength,
}

/// 属性当前值。observe 回放 / PropertyChanged 喂入都用它;转 Lua 值在绑定层做。
#[derive(Clone, Debug, PartialEq)]
pub enum PropValue {
    /// 整数(volume / queue.length)。
    Int(i64),
    /// 浮点秒(position)。
    Secs(f64),
    /// 字符串(state / mode)。
    Str(String),
    /// 歌曲(player.song);`None` = 无当前歌。
    Song(Option<Box<mineral_model::Song>>),
}
```

**observe 两条语义实现**(父文档 §6「照抄 mpv」):

1. **订阅即回放**:`mineral.observe(prop, fn)` 在脚本线程内执行时,从 `ObserveRegistry` 的**末值缓存**取当前值,立刻 `pcall(fn, value)` 一次。缓存初值由 daemon 在 VM 起线程前播一轮全属性快照填充(`PropertyChanged` 批)。
2. **末值合并**:daemon 主循环每 tick diff 出变化属性 → `send(PropertyChanged{key,value})`。脚本线程收到后**先更新末值缓存,再触发该 key 的所有回调**。高频属性(`PlayerPosition`)在 **daemon 侧 diff 阶段节流**:仅当整秒边界变化(`floor(pos/1000)` 改变)才发,避免 20ms tick 每帧推。其余属性按值变化即发(`media.rs:158` 的 diff 范式)。
3. **批次原子性**(父文档 §8):一次 daemon tick 内多个 `PropertyChanged` 用一条 `ScriptEvent::PropertyChanged` 序列连续投递,脚本线程顺序消费;push 给 client 的 `Event::PropertyChanged` 同 tick flush。

`mineral.get(prop)`:同步读末值缓存(脚本线程内 `ObserveRegistry::last(key)`),不触发网络。

### 5.5 看门狗(对外配置 struct,守项目约定)

```rust
/// Lua hook 执行的看门狗阈值。父文档 §13:指令计数 + 墙钟双阈值。
/// 软阈值 → warn(log+toast),硬阈值 → 中断该次执行。阈值来自 sub01 daemon 配置切片。
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct WatchdogConfig {
    /// `set_hook` 的指令计数采样间隔(每 N 条 Lua 指令回调一次检查)。
    instruction_interval: u32,

    /// 软墙钟阈值:单次 hook 执行超过它 → warn,继续跑。
    soft_wall: std::time::Duration,

    /// 硬墙钟阈值:超过它 → `Err` 从 hook 抛出,中断本次执行。
    hard_wall: std::time::Duration,
}

impl WatchdogConfig {
    /// 默认阈值(无配置时):采样 10_000 条指令,软 50ms / 硬 500ms。
    pub fn defaults() -> Self { /* ... */ }

    /// 指令采样间隔。
    pub fn instruction_interval(&self) -> u32 { self.instruction_interval }
    /// 软墙钟阈值。
    pub fn soft_wall(&self) -> std::time::Duration { self.soft_wall }
    /// 硬墙钟阈值。
    pub fn hard_wall(&self) -> std::time::Duration { self.hard_wall }
}
```

> 构造走 `typed-builder`(项目约定);`defaults()` 是内部便捷。**已裁决**:由 **本 crate(mineral-script)提供 `WatchdogConfig::from_daemon_cfg(&mineral_config::DaemonConfig)` 转换**(依赖方向 mineral-script → mineral-config,本 crate 本就依赖 sub01),不让 sub01 反向 builder 出本类型(避免 mineral-config 依赖 mineral-script)。

**中断语义**:每次进入一个 hook 调用前 `lua.set_hook(HookTriggers::every_nth_instruction(interval), move |_lua, _debug| { 检查墙钟; 超硬阈值 Err(...) })`。`set_hook` 回调返回 `Err` 会让当前 Lua 调用栈以错误中止 →被外层 `pcall` 捕获 → 记 `error = mineral_log::chain` + toast,**该 hook 本次失败不致命,不自动禁用**(父文档 §12;连续失败 N 次告警留实现期定)。墙钟基准在每次调闭包前重置(线程局部 `Instant`)。

## 6. track_finished 的 reason 从 player 哪里判定(关键)

reason 不在脚本线程造,在 **daemon 侧事件源**判定后随 `ScriptEvent::TrackFinished` 带进来。映射(对齐 §2 锚点):

| reason | 判定点(file:line) | 现状信号 |
|---|---|---|
| `Eof` | `gapless.rs:254` `check_advance`,finished_seq 前进且非用户操作 | 已有 `spawn_on_played(old, completed=true)` |
| `Skip` | `player.rs:478` `next_song` / `player.rs:451` `prev_or_restart`(进了切歌分支) | 已有 `spawn_on_played(old, completed=false)` |
| `Stop` | `ClientHandle::stop`(`client.rs:201`)/ MPRIS `Stop`(`media.rs:62`)走 `audio().stop()` | 现无「旧曲」语义,需在 stop 前抓 `current_song` |
| `Error` | 取链失败 / decode 失败导致播放无法继续 | 现仅 `play_song` 提交 SongUrl 任务,失败走 TaskOutcome::Failed,**无统一信号** |

**落地做法**:不改散落判定,在四个判定点**各自**调一个新的 `PlayerCore` 内部钩子 `notify_track_finished(song, reason)`,它把 `ScriptEvent::TrackFinished` 投给 `ScriptRuntime::sender`(若 daemon 持有 VM)。`Eof` 在 `check_advance` 拿到 `old_id`/`old` 时投;`Skip` 在 `next_song`/`prev_or_restart` 的切歌分支投;`Stop` 在 stop 前抓当前歌投;`Error` **已裁决推迟 Phase 2**(随 player 级播放失败信号一并补齐——现状 `play_song` 的 SongUrl 失败只进 scheduler 的 `TaskOutcome::Failed`,不经 player 状态;Phase 1 不接)。

> **已裁决**:Phase 1 只保 `Eof` + `Skip` **可靠**;`Stop` **best-effort**;`Error` reason **推迟 Phase 2**(随 player 级播放失败信号补齐)。

`download_completed(song, path)`:现状 `download.rs` 只有**批级** `result_seq` + 每首 `last_ok` 计数,**没有单首完成事件带 path**。**已裁决落地做法**:**扩 `DownloadOutcome` 携带导出 path**(`DownloadOutcome::Downloaded` 变体带上落盘绝对路径),在 `download.rs:329` 每首 `Ok(DownloadOutcome::Downloaded { path, .. })` 分支处把 `(song.clone(), path)` 投 `ScriptEvent::DownloadCompleted`(需 daemon 侧 `ScriptSender`)。这样 path 顺着既有 outcome 流出,不在 `process_target` 重算。

## 7. 实现步骤(依赖顺序,可拆 PR)

> 前置:sub01 已落地(`mineral-config` 能 eval、给出 `DaemonConfig` 切片);sub03 已定 `Event` / `PropName` / `PropValue` 形状。若并行,先用本 crate 内的临时 `Event` 占位,sub03 落地后替换(标注)。

**PR-1 · crate 骨架 + 脚本线程模型(不接 daemon)**
1. workspace 加 `mineral-script` 成员 + `mlua`(lua54/vendored/send)。
2. `message.rs` / `runtime.rs`:`ScriptEvent` / `ScriptCmd` / `ScriptSender` / `ScriptRuntime::spawn`(线程持 `Lua`,消息循环 echo)。
3. `host.rs` / `api/mod.rs` + `install_api` 空壳(挂 `mineral` 空表)。
4. 测试:线程能起停、`send` 不阻塞、drop 干净 join(multi_thread rt,见 §8)。

**PR-2 · 工具墙 + 看门狗(ui/log/watchdog)**
5. `api/ui.rs`:`mineral.ui.toast` → push sink(`Event::Toast`,sub03 形状或占位);`mineral.log.*` → `mineral_log`。
6. `watchdog.rs`:`WatchdogConfig` + `install_hook`;dispatch 调闭包统一经 `pcall` + 墙钟基准重置 + 中断。
7. 测试:死循环 hook 被硬阈值中断、返回 `Err` 被捕获不 panic;toast 进 sink。

**PR-3 · 命令墙 + 事件墙(player / on)**
8. `api/player.rs`:`mineral.player.*` / `download` → `ScriptCmd`。
9. `api/events.rs`:`mineral.on("track_finished"/"download_completed", fn)` 注册 + 分发(reason 转 Lua 字符串)。
10. `dispatch.rs`:`ScriptEvent` → 调注册闭包。
11. 测试:注册的 `on` 收到事件、reason 字符串正确(单测,假投 `ScriptEvent`)。

**PR-4 · 属性墙(get / observe)**
12. `api/observe.rs`:订阅表 + 末值缓存;订阅即回放;`get` 读末值。
13. dispatch 的 `PropertyChanged` 更新缓存 + 触发回调。
14. 测试:订阅即回放、末值合并(连发两值只见末值)、position 节流(单测)。

**PR-5 · 接 daemon(server / cli 注入)**
15. `Server::spawn` 增 `Option<ScriptRuntime>`;`background_loop` 旁每 tick diff 属性 → `runtime.send(PropertyChanged)`(整秒节流 position)。
16. `gapless.rs` / `player.rs` 四个判定点投 `TrackFinished`;`download.rs` 投 `DownloadCompleted`。
17. `serve.rs`:建 VM → `install_api`(挂 `mineral` 活表)→ sub01 eval 配置(顶层 `mineral.on/action` 就地注册进 `ScriptHost`)→ `ScriptRuntime::spawn`(移交 VM)→ 传 `Server::spawn`。**`install_api` 必须在 eval 之前**(对齐本文 §5.2 / §3 接法 B)。
18. e2e:`MINERAL_AUDIO_NULL=1` 起 daemon,喂配置注册 `on("track_finished")` + `observe("player.volume")` + `ui.toast`,断言副作用(见 §8)。

## 8. 测试清单(对齐 docs/testing.md)

单测(随模块 `#[cfg(test)] mod`,**不豁免 lints**,helper/字段带 `///`):

- `runtime`:线程起停 + drop join 无泄漏;`send` 在线程忙时不阻塞 daemon(`assert` 投递立即返回)。
- `watchdog`:`while true do end` hook 在 `hard_wall` 内被中断(返回 `Err`,外层捕获,无 panic);软阈值只 warn 不中断;墙钟基准每次调用重置(连续两次慢 hook 不误判)。
- `events`:`on("track_finished", fn)` 收到 `Eof`/`Skip`/`Stop`/`Error`,fn 拿到的 reason 字符串 == 父文档约定值;未注册的事件 no-op。
- `observe`:**订阅即回放**(注册后立即收到当前值一次);**末值合并**(同 key 连发 `v1,v2,v3`,回调只见 `v3`,但缓存终值正确)——`assert_eq!`;`get` 读到末值缓存。
- `observe 节流`:position 每 20ms 推一次共 50 次跨过 1 整秒,回调只触发 1 次(整秒边界)——`assert_eq!` 计数。
- 命令族:`mineral.player.toggle()` 等产出对应 `ScriptCmd`(脚本线程发出,测试端 drain channel `assert_eq!`)。
- `config check` 风格快照(若本期产出诊断输出):`mineral_test::assert_snap!("脚本 API 注册结果", out)`,**带中文 description**。

e2e(`mineral/tests/script_hooks.rs`,**进 nextest 串行组**):

- 复用 `daemon_lifecycle.rs` 的 `Daemon::spawn_null` 范式(`CARGO_BIN_EXE_mineral` + `MINERAL_AUDIO_NULL=1`,隔离 XDG)。新增隔离的 `config.lua`(写进临时 `XDG_CONFIG`)注册 `mineral.on("track_finished", ...)` + `mineral.observe("player.volume", ...)` + `mineral.ui.toast(...)`。
- 断言:经 IPC 改音量 → observe 回调触发(经 `ui.toast` 回推的 `Event::Toast` 在 client 侧可见,或脚本写 `mineral.log` 落日志文件后 grep)。
- **nextest 串行组**:在 `.config/nextest.toml` 加 override `filter = "binary(script_hooks)"` → `test-group = "daemon-e2e"`(与既有 daemon e2e 共组,防并发抢 socket)。
- **multi_thread rt**(MEMORY「真实 IO/audio engine 测试需 multi_thread」):任何起真 daemon / audio engine 的 tokio test 用 `#[tokio::test(flavor = "multi_thread", worker_threads = …)]`,否则全仓并发 flaky。脚本线程本身是裸 `std::thread`,不受 rt flavor 影响,但 daemon 主循环要 multi_thread。
- **doctest**:对外 `pub` API(`install_api` / `ScriptRuntime`)的 `///` 示例走 `cargo td`(nextest 不跑 doctest)。

CI:`mlua` vendored 一次性编译进链(`libasound2-dev` 先例);headless 照常(audio null 降级)。

## 9. 依赖 sub03 的字段(只读,以 sub03 为准)

本期生产 `mineral_protocol::Event` 实例,需 sub03 提供(父文档 §8):
- `Event::Toast { kind: ToastKind, content: String, id: Option<String> }`(同 id 替换)。
- `Event::PropertyChanged { prop: mineral_protocol::PropName, value: mineral_protocol::PropValue }`(observe 的协议面)。
- `ToastKind`(`Info`/`Warn`/`Error`)。
- `mineral_protocol::PropValue`(wire 值载体;本 crate 内部同名的 `PropValue` 在边缘转成它——二者跨 crate 不冲突)。

若 sub03 未就绪,PR-2 起用本 crate 内 `pub(crate)` 占位 enum,并在代码注释标 `// TODO(sub03): 替换为 mineral_protocol::Event`,落地后一次性替换 + 删占位。

## 10. 验收判据与风险

**验收判据**(父文档 §15 Phase 1):
- `on("track_finished", fn)` 在 `Eof` / `Skip` 下可靠触发,fn 拿到正确 reason 字符串。
- `observe("player.volume", fn)` 满足订阅即回放 + 末值合并两条语义;`get("player.volume")` 读到当前值。
- `ui.toast(...)` 经 push 通道回到 client 可见(e2e 绿)。
- headless(`MINERAL_AUDIO_NULL=1`)能跑 hook,daemon 照常 bind/serve/graceful shutdown。
- 死循环 hook 被看门狗中断,不卡 daemon 主循环(播放/IPC 继续响应)。
- `cargo clippy --workspace --all-targets -- -D warnings` 绿(含 800 行/300 行约束、`missing_docs`)。

**风险与缓解**:
- **`Error`/`Stop` reason 缺统一信号**(§6):**已裁决** Phase 1 只保 `Eof`/`Skip` 可靠、`Stop` best-effort,`Error` reason **推迟 Phase 2**(随 player 级播放失败信号补齐);不阻塞 Phase 1 交付。
- **`download_completed` 需单首 path**(§6):**已裁决** 通过**扩 `DownloadOutcome` 携带导出 path** 解决(`Downloaded` 变体带 path),path 顺 outcome 流出、不在 `process_target` 重算。
- **脚本线程 ↔ tokio runtime 跨界**:裸 `std::thread` 用 `mpsc` + `blocking_recv`;push sink 用 `UnboundedSender`(非阻塞)。命令回 daemon 经 channel 串行执行,避免脚本线程并发改 `PlayerCore` 状态。
- **看门狗中断 + `vendored` 信任级**:`set_hook` 只能拦 Lua 字节码层,**C 调用(io/os 阻塞)拦不住**(nvim 同款);Phase 1 接受(父文档 D10 无沙箱),文档注明慢 C 调用仍可能卡脚本线程(但卡的是脚本线程,不是 daemon 主循环 —— 这正是专线程模型的价值)。
- **顶层副作用多次执行**(父文档 D8):本 crate 只在 daemon 进程持活 VM;TUI/CLI 的 `on/action` 为 no-op(sub01 stub),本 crate 不涉及。
- **mlua 声明**:引用 workspace 条目(canonical 声明见 sub01,features 并集含 `send`/`lua54`/`vendored`/`serialize`,版本落地时取最新 stable);`send` feature 是 VM 跨线程移交脚本线程的前提,务必在 canonical 条目里开(已纳入 sub01 并集)。本 subspec 不自行钉版本。
