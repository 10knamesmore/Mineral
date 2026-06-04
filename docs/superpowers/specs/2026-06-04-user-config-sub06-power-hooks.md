# Sub06:强力位——同步拦截 / 事件总线 / spawn / search(Phase 3,方向级)

> 父设计:`2026-06-04-user-config-lua-design.md`(§9 P3 行、§13 看门狗、§12 降级、§10 协议护栏)。
> 本文是**方向级 subspec**:Phase 3 的四个强力 API 风险高、依赖未落地的 Phase 1/2 基建,故只定边界、签名草案与不变量,不展开逐行实现计划。落地前需以独立 brainstorm 收敛各自细节。
> 本文档按项目约定不纳入版本控制。

## 1. 范围与不做

**本 subspec 负责(Phase 3,父文档 §9 P3 四行):**

- `hook("before_play" / "before_download", fn)`——**同步拦截点**,fn 可改 URL / 音质 / 跳过,带 `cont()` / `defer()` 与超时放行语义。
- `emit(name, payload)` / `on_message(name, fn)`——**自定义事件总线**,client 间经 daemon 中转(MPD client-to-client 逃生口)。
- `mineral.spawn(args, opts, fn)`——**结构化异步子进程**(`tokio::process` 桥,可中止 handle)。
- `library.search(q, opts, fn)`——**异步回调形**搜索(`MusicChannel::search_songs` 等的脚本投影)。

**明确不做(交界):**

- 四承重墙最小集(`player.*` / `get` / `observe` / `on` / `action` / `ui.toast` / `log.*`)、push 通道泛化(`Event::Toast` / `PropertyChanged`)、看门狗线程、daemon VM 宿主、脚本线程 = **Phase 1 / sub04(daemon VM)**。本文**复用**它们,不重新定义;凡引用 `脚本线程` / `Lua` / `观测分发` / `看门狗` 均指 Phase 1 已立的设施。
- `action` / `bind` / `KeyTriggered` / `KeyContext` / `queue.*` / `library.playlists|tracks|love` / `store.*` / `timer.*` = **Phase 2 / sub05**。`library.search` 与 sub05 的 `library.*` 同模块,签名风格须对齐(见 §6 接口)。
- 配置 schema / loader / merge / `default.lua` / `Config` 强类型 = **Phase 0 / sub01~sub03**。本文只新增 `daemon` 段下若干**熔断阈值字段**(§5),其类型与 getter 由 sub02 的 `daemon` 段统一承载,本文只声明字段名与默认值。
- 安全沙箱:不做(父 §1 非目标、D10)。`spawn` 任意命令、`io`/`os` 全开。

## 2. 现状锚点(file:line)

拦截点与中转链的真实落点,Phase 3 在这些缝里插桩:

- **播放 URL 解析链**:`crates/mineral-server/src/player.rs:290`(`play_song`)→ 本地命中走 `crate::resolve::resolve_local`(`player.rs:312`),否则提交 `ChannelFetch(SongUrl)` 任务(`player.rs:375`);URL 真正就绪在 `handle_play_url_ready`(`player.rs:607`)→ `download::play_capturing`(`player.rs:630` / `crates/mineral-server/src/download.rs:218`)。**`before_play` 的拦截窗口 = 解析出 `PlayUrl` 后、`audio.play(...)` 前**——即 `play_capturing`(`download.rs:218`)入口、与 `local_play_url` 命中后(`player.rs:366`)两处。
- **下载链**:`crates/mineral-server/src/download.rs:73`(`download_song`)→ 取直链 `channel.song_urls(...)`(`download.rs:88`)→ 算导出路径 → `stream_to_file`(`download.rs:114`)。**`before_download` 拦截窗口 = `song_urls` 返回后、`stream_to_file` 前**(`download.rs:96` 一带,改 URL / 跳过)。worker 编排在 `download.rs:268` / `process_target` `download.rs:287`。
- **channel 搜索**:`crates/mineral-channel/core/src/lib.rs:35-39`(`search_songs` / `search_albums` / `search_playlists`,均 `async fn ... -> Result<Vec<_>>`)。netease 实现 `crates/mineral-channel/netease/src/channel.rs:227`(`song_urls`)。`library.search` 经 `PlayerCore::channel_for`(`player.rs:173`)路由到对应 channel。
- **IPC 现状(关键约束)**:`crates/mineral-server/src/serve.rs:68` `handle_connection` 是**严格 1:1 顺序** req-resp;`serve.rs:36` **单 client busy 拒绝**。`emit` / `on_message` 的 client 间中转**必须先有 Phase 1 的 `Event` 异步下推通道**(同连接交错,父 §8 / §10.2),当前协议(`crates/mineral-protocol/src/message.rs:83` `Request` / `:177` `Response`)无 server→client 主动推送 variant。
- **现成的"推一条给 client"雏形**:`player.rs:208` `push_notice` → `TaskEvent::Notice`(`crates/mineral-task/src/event.rs:69`),经 `drain_client_events`(`player.rs:157`)被动拉取。Phase 1 会把它泛化为主动 `Event` 推送,`on_message` 复用该通道。
- **PlayerCore 注入面**:`player.rs:109` `spawn` 是脚本运行时句柄的注入入口;`channel_for`(`player.rs:173`)/ `audio`(`player.rs:151`)/ `media_cache`(`player.rs:178`)是 hook 回调需要触达的能力。
- **tokio 能力**:根 `Cargo.toml:106` `tokio = features = ["full"]` → `tokio::process` 已在依赖面,`spawn` 无需新增 feature。

## 3. 新增 / 修改文件清单

落点全在 **sub04 新建的 `mineral-script` crate** 内(本文不新建 crate,只在其下添模块),守"单文件 ≤800 行、`mod.rs`/`lib.rs` 不写逻辑"。

| 文件 | 职责 | 预估规模 |
|---|---|---|
| `crates/mineral-script/src/hooks.rs`(新) | `before_play` / `before_download` 同步拦截注册表 + `HookDecision`(cont / defer / skip / rewrite)+ 超时放行裁决。**纯决策逻辑**,不碰 audio/http | 250~350 行 |
| `crates/mineral-script/src/bus.rs`(新) | `emit` / `on_message` 事件总线:Lua payload ↔ 结构化 `BusMessage` 编解码、订阅表、经 daemon `Event` 通道扇出 | 200~300 行 |
| `crates/mineral-script/src/proc.rs`(新) | `mineral.spawn`:`tokio::process::Command` 桥、`ChildHandle`(kill / pid / 状态)、stdout/stderr 回调投递到脚本线程 | 250~350 行 |
| `crates/mineral-script/src/search.rs`(新) | `library.search`:把 `MusicChannel::search_*` 投影为 Lua 异步回调形;结果序列化为 Lua table | 150~250 行 |
| `crates/mineral-server/src/hook_bridge.rs`(新) | server 侧**插桩点**:`play_song` / `download_song` 调脚本线程跑 hook、应用 `HookDecision`。**唯一改 server 行为的地方**,薄 | 150~250 行 |
| `crates/mineral-protocol/src/event.rs`(改) | 给 sub03(Phase 1)建的 `Event` enum **追加** `Event::BusMessage { name, payload }`(client 间中转面);若 sub03 尚未建 `Event`,本文标记为**依赖项**不抢建 | +~15 行 |
| `crates/mineral-config` `daemon` 段(改,归 sub02) | 新增熔断/超时字段(§5),本文只声明名与默认 | +~20 行(在 sub02) |
| `crates/mineral-server/src/player.rs`(改) | `play_song` / `play_capturing` 调用前插 `hook_bridge::before_play`;`PlayerCore::spawn` 注入脚本运行时句柄 | 改动 ≤30 行 |
| `crates/mineral-server/src/download.rs`(改) | `download_song` 取链后插 `hook_bridge::before_download` | 改动 ≤20 行 |

> `mineral-script/src/lib.rs`(sub04 建)仅 `pub use` 上述模块,**不写逻辑**。

## 4. 关键类型与签名(Rust,遵守项目约定)

以下为方向级签名草案,不可 `unwrap`/`expect`/`panic`/`as`,错误走 `color_eyre`;对外配置 struct 私有字段 + `#[non_exhaustive]` + getter。

### 4.1 同步拦截:`HookDecision` + 裁决

```rust
/// 一次同步拦截 hook(`before_play` / `before_download`)的裁决结果。
///
/// 脚本侧用 `cont()` / `defer()` / 改字段 / `skip()` 表达,宿主收敛成本枚举。
/// 看门狗超时未 `cont` 视为 `Continue`(放行)并 warn(父 §13)。
#[derive(Debug, Clone)]
pub enum HookDecision {
    /// 放行,沿用原 `PlayUrl` / 原下载直链。
    Continue,

    /// 用脚本改写后的播放 URL / 音质继续(多源 fallback 场景)。
    Rewrite(Box<RewriteSpec>),

    /// 跳过本次播放 / 下载(脚本判定不可用),宿主据此降级:播放跳下一首 / 下载记 fail。
    Skip {
        /// 跳过原因,进 toast + 日志,人读即可。
        reason: String,
    },
}

/// 脚本对 `before_play` / `before_download` 的改写意图(结构化,边缘才序列化进 Lua)。
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct RewriteSpec {
    /// 改写后的远端直链;`None` = 不改 URL。
    new_url: Option<url::Url>,

    /// 改写后的目标音质;`None` = 不改音质。
    new_quality: Option<mineral_model::BitRate>,
}

impl RewriteSpec {
    /// 改写后的直链(只读)。
    pub fn new_url(&self) -> Option<&url::Url> {
        self.new_url.as_ref()
    }

    /// 改写后的音质(只读)。
    pub fn new_quality(&self) -> Option<mineral_model::BitRate> {
        self.new_quality
    }
}
```

```rust
/// 一次同步拦截的入参快照(只读,跨脚本线程边界 move)。
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct HookContext {
    /// 触发 hook 的歌曲。
    song: mineral_model::Song,

    /// 宿主解析出的原始播放 URL(`before_play`)或下载直链(`before_download`)。
    original: mineral_model::PlayUrl,
}

impl HookContext {
    /// 触发歌曲(只读)。
    pub fn song(&self) -> &mineral_model::Song {
        &self.song
    }

    /// 原始 URL / 音质(只读)。
    pub fn original(&self) -> &mineral_model::PlayUrl {
        &self.original
    }
}

/// 同步跑一次某类 hook,带墙钟超时;超时放行 + warn(父 §13)。
///
/// # Params:
///   - `kind`: 拦截点类别(before_play / before_download)
///   - `ctx`: 入参快照
///   - `timeout`: 软超时,来自 daemon 配置段(§5)
///
/// # Return:
///   裁决结果;无注册 hook → `Ok(HookDecision::Continue)`;Lua 运行错经 pcall
///   捕获后**也**返回 `Continue`(失败不致命,父 §12),仅日志 + toast。
pub async fn run_intercept(
    &self,
    kind: HookKind,
    ctx: HookContext,
    timeout: std::time::Duration,
) -> color_eyre::Result<HookDecision> {
    // 实现:把 ctx send 到脚本线程,await 一个带 timeout 的 oneshot;
    // 超时 → Continue + warn;通道断 → Continue + warn。
    todo!("方向级:实现期定 oneshot + tokio::time::timeout")
}
```

`cont()` / `defer()` 语义(脚本侧):`defer()` 把裁决权交回一个 deferred token,宿主在 `timeout` 内等其 `cont(decision)`;不调即超时放行。同步拦截**只允许**这两条出口,其它 Lua 异常一律当 `Continue`。

### 4.2 事件总线:`emit` / `on_message`

```rust
/// client 间事件总线的一条消息(结构化;Lua payload 在边缘序列化)。
///
/// `payload` 走 `serde_json::Value` 而非 String:父约定"rust 内部一定优先结构化,
/// 边缘适配层再序列化"。Lua table ↔ Value 的转换由 `bus.rs` 在 VM 边界做。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BusMessage {
    /// 事件名(命名空间约定,如 `"my.refresh"`)。
    pub name: String,

    /// 任意结构化负载。
    pub payload: serde_json::Value,
}
```

```rust
// crates/mineral-protocol/src/event.rs —— 追加到 sub03(Phase 1)已建的 Event enum:
/// 自定义事件总线消息:一个 client `emit` 后,daemon 扇出给所有 `on_message` 订阅者
/// 与其它 client(同连接交错下推,父 §8 / §10.2)。
BusMessage {
    /// 事件名。
    name: String,
    /// 结构化负载(JSON 形,跨 codec 稳定)。
    payload: serde_json::Value,
},
```

> **依赖前提**:`Event` 异步下推通道由 Phase 1 建。本文若先于 Phase 1 落地,`emit`/`on_message` 退化为**仅 daemon 内 Lua↔Lua**(单 VM 自发自收),client 间中转留待 `Event` 通道就绪——这是合法的渐进降级,不阻塞。

### 4.3 结构化子进程:`mineral.spawn`

```rust
/// `mineral.spawn` 的选项(私有字段 + builder/默认;Lua 边缘构造)。
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct SpawnOptions {
    /// 工作目录;`None` = 继承 daemon cwd。
    cwd: Option<std::path::PathBuf>,

    /// 追加 / 覆盖的环境变量。
    env: Vec<(String, String)>,

    /// 是否捕获 stdout/stderr 回调(false 时直继承,省内存)。
    capture_output: bool,
}

impl SpawnOptions {
    /// 工作目录(只读)。
    pub fn cwd(&self) -> Option<&std::path::Path> {
        self.cwd.as_deref()
    }
    // env() / capture_output() getter 同理省略
}

/// 一个在跑的子进程句柄(可中止)。脚本侧 `handle:kill()` / `handle:pid()`。
pub struct ChildHandle {
    /// 子进程 pid(已 spawn 才有)。
    pid: Option<u32>,

    /// kill 信号发送端(脚本线程持有,跨 await 安全)。
    kill_tx: tokio::sync::oneshot::Sender<()>,
}

impl ChildHandle {
    /// 中止子进程(SIGKILL via tokio kill);已退出则 no-op。
    pub fn kill(self) {
        let _ = self.kill_tx.send(());
    }
}

/// spawn 一个结构化异步子进程,退出后回调 fn(结果)。
///
/// # Params:
///   - `program`: 可执行文件
///   - `args`: 参数(结构化,非拼 shell 串)
///   - `opts`: 选项
///
/// # Return:
///   可中止句柄;`spawn` 本身失败(可执行不存在等)→ `Err`,脚本侧回调收到错误。
pub fn spawn_child(
    &self,
    program: &std::ffi::OsStr,
    args: &[std::ffi::OsString],
    opts: SpawnOptions,
) -> color_eyre::Result<ChildHandle> {
    // 实现:tokio::process::Command,spawn 后 tokio::spawn 监听 wait() 与 kill_rx,
    // 完成/被杀都把结构化结果 send 回脚本线程触发 Lua 回调。
    todo!("方向级:实现期定 Command 组装 + select! 监听")
}
```

> **spawn 输出投递方式(行缓冲流式 vs 退出时整块回传)标注「Phase 3 实现期裁决」**,不在本轮定;本文只约定"结构化结果 send 回脚本线程触发 Lua 回调"的方向。

### 4.4 异步搜索:`library.search`

```rust
/// `library.search` 的范围(对齐 MusicChannel 的搜索族)。
#[derive(Debug, Clone, Copy)]
pub enum SearchScope {
    /// 单曲(`MusicChannel::search_songs`)。
    Songs,
    /// 专辑。
    Albums,
    /// 歌单。
    Playlists,
}

/// 异步搜索,完成后回调 Lua fn(命中列表)。**只读**,符合"不放权算法"取向(父 §1)。
///
/// # Params:
///   - `channel`: 路由到的源(经 PlayerCore::channel_for)
///   - `scope`: 搜索范围
///   - `query`: 关键词
///   - `page`: 分页(对齐 mineral_channel_core::Page)
///
/// # Return:
///   命中;channel 不支持 → `Err(Error::NotSupported)`,脚本回调收到空 + 原因。
pub async fn run_search(
    channel: &dyn mineral_channel_core::MusicChannel,
    scope: SearchScope,
    query: &str,
    page: mineral_channel_core::Page,
) -> mineral_channel_core::Result<SearchHits> {
    // 实现:match scope → 调对应 search_* → 收敛进 SearchHits(结构化,边缘转 Lua table)
    todo!("方向级")
}
```

> `SearchHits` 与 Lua↔Song table 编解码 **owner 已裁决归 sub05**(其 §4.7);本文 `library.search` 复用同一份,不另造。见 §6 接口。

## 5. 新增配置字段(归 sub02 `daemon` 段)

本文只声明字段与默认值,类型 / getter 由 sub02 统一实现:

```lua
daemon = {
  -- 同步拦截 hook 的软超时(ms):超时放行 + warn(父 §13 / §9 P3 "必带熔断")。
  hook_intercept_timeout_ms = 2000,
  -- 同步拦截连续失败 N 次后告警(不自动禁用,父 §12)。
  hook_fail_warn_threshold = 5,
  -- spawn 子进程的并发上限(防脚本 fork 炸);0 = 无限。
  spawn_max_concurrent = 8,
}
```

不暴露:总线扇出批大小、search 默认分页(内部常量)。

## 6. 跨 subspec 共享接口(一致性检查锚点)

- `mineral_script::HookDecision` / `RewriteSpec` / `HookContext` / `HookKind`——本文定义,sub04(daemon VM)消费(注册表挂在它建的脚本线程上)。
- `mineral_script::run_intercept` / `spawn_child` / `run_search`——挂在 sub04 的脚本运行时句柄上(本文不建该句柄,只约定方法签名)。
- `mineral_protocol::Event::BusMessage`——**追加**到 sub03 建的 `Event` enum(本文不抢建 `Event` 本体)。
- `SearchHits` / Lua↔Song table 编解码——**已裁决 owner 归 sub05**(其 §4.7,由 sub05 PR-H 一并定义);本文 `library.search` 复用同一份,不另造。
- `mineral_server::hook_bridge`——本文新建,是 server 唯一插桩面;`PlayerCore::spawn`(`player.rs:109`)注入脚本句柄。
- `daemon` 配置段三字段(§5)——归 **sub02** 实现,本文只声明。

## 7. 实现步骤(依赖顺序,可拆 PR)

> 强前提:Phase 1(sub04 daemon VM + 脚本线程 + 看门狗 + `Event` 通道)**已合并**。Phase 3 不可早于它。

1. **PR-A `hook_bridge` 骨架 + before_play(无改写)**:在 `player.rs` 播放 URL 解析后插一个**恒 `Continue`** 的拦截调用,接通脚本线程往返 + 超时放行裁决(§4.1)。先不暴露 Lua API,验证插桩不破坏既有播放(`downloads_then_plays_from_download` 等回归绿)。
2. **PR-B 拦截改写 + skip + Lua `hook(...)`**:落 `RewriteSpec` / `Skip`,接 `before_download`(`download.rs:96`)。暴露 `cont()`/`defer()`/`skip()`。多源 fallback 脚本可写(交付判据)。
3. **PR-C `mineral.spawn`**:`proc.rs` + `ChildHandle`,`spawn_max_concurrent` 闸。独立于 hook,可并行 PR-B。
4. **PR-D `library.search`**:`search.rs`,复用 sub05 的 Song table 编解码(若 sub05 未合并,先内联一份临时编解码,sub05 合并后归一)。
5. **PR-E `emit`/`on_message`**:`bus.rs` + `Event::BusMessage`。**最依赖** Phase 1 `Event` 通道——若通道未就绪,先落"daemon 内 Lua↔Lua"降级版(§4.2 注),client 间中转单独 PR。

每个 PR 独立可回归;PR-A 是其余的地基,必须最先。

## 8. 测试清单(对齐 docs/testing.md)

- **拦截放行不变性(核心回归)**:无注册 hook 时 `play_song` / `download_song` 行为与今日**逐字节一致**——复用 `crates/mineral-server/src/player.rs:1278` `downloads_then_plays_from_download` 与 `download.rs:415` `download_does_not_populate_cache`,插桩后必须仍绿。`#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`(走真实 TCP + audio engine,memory 教训:单线程 rt flaky)。
- **超时放行**:注册一个 `defer()` 后**永不 cont** 的 hook,断言 `run_intercept` 在 `hook_intercept_timeout_ms` 后返回 `Continue` + 产生一条 warn(assert on `mineral_log::chain` 文本 / toast)。`tokio::time` 可 `start_paused = true` 确定性推进。
- **改写 / skip**:`before_play` 注册改 URL 的 hook → 断言 `play_url` 被替换;注册 `skip` → 断言播放跳过(下一首 / 不发声)。
- **spawn**:`spawn_child("echo", ["hi"])` → 回调收到退出码 0 + stdout `"hi"`;`kill()` 在跑的 `sleep` → 回调收到被中止。`multi_thread` rt(真起子进程)。`spawn_max_concurrent` 上限单测。
- **search**:mock channel(`mineral_test::mock::UrlChannel` 风格,扩一个 `search_songs` 返固定结果)→ `run_search(Songs)` 命中;`NotSupported` channel → 回调收空 + 原因。
- **bus**:`emit("x", {...})` → 同 VM `on_message("x")` 收到结构化 payload;`Event::BusMessage` 序列化 round-trip(bincode + JSON value)单测。
- **e2e(daemon 串行组)**:`crates/mineral/tests/daemon_lifecycle.rs` 同组(`.config/nextest.toml` `daemon-e2e` `max-threads=1`)+ `MINERAL_AUDIO_NULL=1`:headless 起 daemon,经 config.lua 注册 `before_play` 改写 + 一个 `spawn`,断言整链不崩、降级语义保住。
- **快照**:`mineral config check` 对含 P3 API 的配置输出走 `assert_snap!` 带中文 `description`(父 §14)。
- 所有测试不豁免 lints:无 `unwrap`/`expect`/`indexing_slicing`,helper/字段带 `///`。

## 9. 验收判据

- **多源 fallback 脚本可写**(父 §15 Phase 3 判据):用户在 config.lua 写 `hook("before_play", fn)`,fn 探测原 URL 不可用时改写到备用源 URL,播放无缝切换。
- 无配置 / 无 hook 时,播放 / 下载 / 搜索行为与今天**完全一致**(回归绿)。
- 慢 / 死循环拦截 hook **不卡播放**:超时放行 + warn,播放照常推进。
- `spawn` 的子进程可被 `kill()` 中止,daemon 不泄漏僵尸进程。
- `backend = "null"` / cover-disabled / netease 未登录等既有降级链**不被本文任何 API 破坏**(父 §12)。

## 10. 风险

| 风险 | 缓解 |
|---|---|
| **同步拦截卡播放主路径**——`before_play` 在切歌热路径上,慢 hook 直接听感卡顿 | 软超时放行(§4.1 / §5);拦截只在脚本线程跑,daemon 主 loop 经 oneshot await 不持播放锁;默认超时保守(2s),文档警示"拦截要快" |
| **Phase 1 `Event` 通道未就绪**,`emit`/`on_message` client 间中转无处落 | §4.2 渐进降级:先落 daemon 内 Lua↔Lua,client 中转随 `Event` 通道单独 PR;不阻塞其余三个 API |
| **`spawn` fork 炸 / 僵尸进程** | `spawn_max_concurrent` 闸(§5);`ChildHandle` 退出/被杀都 `wait()` 收尸;tokio kill = SIGKILL |
| **改写 URL 绕过缓存/解析不变量**——`RewriteSpec` 改了 URL 后,`resolve_local`(`resolve.rs:34`)的本地优先 / capture 入缓存(`download.rs:218`)语义可能错乱 | 拦截窗口选在**解析后**(`player.rs:366` / `download.rs:96`),改写只替换最终 URL,不回退重跑 resolve;capture 仍按原 song_id 入缓存,改写 URL 的内容是否一致由脚本自负(文档注明)。**改写 URL 的缓存污染细则(是否禁止改写结果入缓存 / 入缓存的 key 策略)标注「Phase 3 实现期裁决」,不在本轮定** |
| **search 被误用为算法发现入口**——破坏"不放权算法"取向(父 §1) | `library.search` 只是 `search_*` 的薄投影、只读;不暴露日推/相似/榜单类端点(本就不在 `MusicChannel` trait 上) |
| **同步拦截与看门狗指令计数冲突**——拦截 hook 可能合法地慢(等网络探测) | 拦截走**墙钟超时**而非指令计数熔断(§4.1);指令计数熔断仍适用于纯计算死循环,二者正交。**看门狗双机制(墙钟 vs 指令计数)对拦截 hook 的优先级与分流细则标注「Phase 3 实现期裁决」,不在本轮定** |
| **P3 依赖链长(Phase 1+2 都得先落)** | 本文方向级,不锁死细节;PR-A 地基先行,其余可在 Phase 1 合并后陆续 brainstorm 收敛 |
