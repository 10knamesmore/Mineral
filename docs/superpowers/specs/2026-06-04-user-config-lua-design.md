# Mineral User Config 系统设计(Lua)

> 状态:brainstorm 已收敛(2026-06-04),待用户终审 → 实现计划。
> 本文档按项目约定不纳入版本控制。

## 0. 一句话

用 Lua 做用户配置:`default.lua` 经 `include_str!` 内置、启动期与用户 `config.lua` 深合并后落成强类型 Rust `Config`;声明式配置(主题/键位/音频/缓存等)占 90%,可编程层(事件 hooks / 自定义动作)由 **daemon 内唯一常驻 Lua VM** 承载;全程带 LuaCATS 类型标注。

## 1. 目标 / 非目标

**目标**

- 用户可在 `~/.config/mineral/config.lua` 覆盖运行期配置;不写配置时行为与今天完全一致。
- 完整可编程:`mineral.on(event, fn)` 事件自动化、`mineral.action(name, fn)` 自定义动作,headless(无 TUI)也能跑。
- 强类型:Rust 侧 serde 强类型 `Config`;Lua 侧 LuaCATS 注解 + `---@meta` stub,编辑器有补全/类型检查。
- 为远期 web/nvim client 留好协议护栏(见 §10),但不为其新增当下工程量。

**非目标**

- 不做安全沙箱(`io`/`os`/`require` 全开,信任级别 = `.zshrc` / nvim `init.lua`)。
- 不做 nvim 式"瘦 client"重构:视图态不进 daemon。
- 不在本期做热重载(Phase 2 末再说)、不做 web/nvim frontend。
- 不放权算法/不做歌单写操作(项目既有取向,配置系统不改变它)。

## 2. 核心决策记录

| # | 决策 | 理由(摘要) |
|---|---|---|
| D1 | 配置语言用 **Lua**(mlua,`lua54 + vendored`,版本以落地时最新为准),清理孤儿依赖 `config_rs`(`Cargo.toml:41`,全仓零使用) | 用户既定方向;vendored 免去用户 C 工具链;rlua 已是 mlua 薄封装 |
| D2 | **默认配置也是 Lua**:`default.lua` 经 `include_str!` 内置,启动期 eval(不可能 const 求值,Lua VM 是运行期物) | 单一真相源 + 自文档;`mineral-config` 现有两个 `pub const`(`lib.rs:7,10`)退役 |
| D3 | 合并语义:**Lua 层深合并**(用户表覆盖默认表,table 按键深合并、**数组整体替换**),合并后整表一次 `from_value::<Config>` | 用户配置天然 partial;缺字段回落默认 |
| D4 | **完整可编程**(事件 hooks),但 VM **唯一且落 daemon** | "关 client 不杀常驻 server"的愿景 ⇒ daemon 是持久大脑;双 VM 有双触发/双沙箱/心智割裂问题,已否决 |
| D5 | **状态线**:领域态(播放/队列/音量/登录/缓存)归 daemon,视图态(滚动/选中/搜索/浮层/全屏)留 TUI | nvim 反面教材:视图态全进 core 导致 multihead 多年难产;Mineral 不重蹈 |
| D6 | 键位 = **声明式重映射**(键 → 具名动作,TUI 解析)+ **任意函数经键转发**(TUI 发 `KeyTriggered{key, ctx}`,daemon VM 执行) | 视图动作需要 TUI 本地态;领域动作/自定义 Lua 在 daemon;nvim 证明"键的物理来源"与"解析执行地"可分离 |
| D7 | API 形状 = mpv 四承重墙:**属性树 observe + 离散事件 on(带 reason)+ 结构化命令 + 具名动作 action** | mpv 生态实证:90% 脚本调用落在前三族;状态值不造 bespoke 事件 |
| D8 | 加载拓扑:**loader 是库,谁都能 eval;活 VM 只在 daemon**。TUI 启动自 eval 拿声明切片(首帧即有主题,`on`/`action` 为 no-op stub);CLI 无 daemon 时同理 | 兼得首帧不闪 + hook 单一权威 + CLI 离线可用;代价:配置顶层副作用执行多于一次,文档注明 |
| D9 | 错误 UX:**永不拒绝启动**。解析失败 → 落默认 + toast 报 file:line;hook 运行错 → pcall 捕获 + toast + 继续 | 与 audio-null / cover-disabled 降级语义一致 |
| D10 | 无安全沙箱,有**可用性看门狗**:hook 在专用线程跑,指令计数/超时熔断,慢 hook 不卡播放 | nvim/wezterm 均不沙箱;真实风险是慢 hook 卡 daemon(nvim 同款痛点) |
| D11 | **LuaCATS 全量标注**:`default.lua` 字段注解 + host API `---@meta` stub;`mineral config init` 生成模板 + `.luarc.json` | 用户编辑器内有 LSP 体验;Rust `Config` 是类型真相源,stub 是影子 |
| D12 | **client 配置入 client 命名空间**:`tui = { theme, keys }`,顶层只留 daemon/共享核心段;未来 in-repo client 平行加段(`web = {}`) | TUI 无协议特权,只有"in-repo 打包特权";第三方 client(如 nvim 插件)的配置活在自己生态(init.lua),不进 mineral config.lua——`deny_unknown_fields` 因此恒成立 |

## 3. 配置加载管线

```
include_str!("lua/default.lua") ──┐  (eval, 必定成功——有测试守)
                                   ├─ Lua 层深合并(用户覆盖默认)
~/.config/mineral/config.lua ─────┘     │ 用户文件缺失 ⇒ 跳过,纯默认
                                         ▼
                          lua.from_value::<Config>(merged)   (serde, deny_unknown_fields)
                                         │ 失败 ⇒ 落纯默认 + 报错(D9)
                                         ▼
                              强类型 Config(私有字段 + getters + #[non_exhaustive])
                                         │
              ┌──────────────────────────┼──────────────────────────┐
              ▼                          ▼                          ▼
        daemon 切片                  TUI 切片                   CLI 切片
   (audio/cache/download/      (tui 段:theme/keys)        (按需,离线可自 eval)
    sources/daemon 段)
              │
              └─ daemon eval 后**保留 VM**:hooks/actions 注册表生效(§6)
                 TUI/CLI eval 完即丢 VM(mineral.on/action = no-op stub)
```

要点:

- 合并在 Lua 层做(一段内置 Lua merge 函数),合并完只跨界一次。
- **数组整体替换**不做元素级合并(语义无歧义);schema 设计尽量用 map 减少数组字段。
- Lua `nil` 即"缺省",**无法表达"显式置空"**;需要"关闭"语义的字段用 `false` 或显式枚举值(如 `proxy = false` 表示禁用),schema 设计时逐字段确认。
- 配置路径:`mineral_paths::config_dir()`(`xdg.rs:40-42`)+ `config.lua`。
- 优先级:CLI flag > `MINERAL_*` env > config.lua > default.lua。env 既有项(`MINERAL_SOCKET_DIR`、`MINERAL_DOWNLOAD_DIR`、`MINERAL_AUDIO_NULL`)保持兼容。

## 4. 类型:Rust 真相源 + LuaCATS 影子

- Rust:`Config` 及子结构 serde `Deserialize` + `deny_unknown_fields`;私有字段 + `#[non_exhaustive]` + derive-getters(项目对外配置 struct 约定;构造方即 serde,不需要 builder——约定中 builder 针对"调用方手工构造"场景,此处不适用,作记录)。
- Lua:
  - `default.lua` 每个字段带 LuaCATS 注解(`---@class mineral.Config` / `---@field volume integer # 0-100`)。
  - host API 一套 `---@meta` stub(`mineral.player.toggle`、`mineral.observe` 等签名 + 文档),随仓库维护、随程序分发。
  - `mineral config init`:生成 `~/.config/mineral/config.lua` 模板 + `.luarc.json`(workspace.library 指向 stub 安装目录)。
- 防漂移:
  - "default.lua eval → `from_value::<Config>` 必须成功"单测(字段少了/型错当场红);
  - `deny_unknown_fields`(default.lua 多写字段当场红);
  - 注解文本漂移短期靠 review 纪律,远期可选:从 Rust 类型 codegen LuaCATS stub。

## 5. 配置 schema 草案(v1 声明面)

```lua
---@type mineral.Config
return {
  -- client 段(D12):TUI 作为 in-repo client 的专属命名空间;协议上无特权,仅打包特权。
  -- 未来 in-repo client 平行加段(web = {});第三方 client 配置不进本文件。
  tui = {
    theme = {
      -- 14 个 color token(现硬编码 theme.rs:61-74)+ 3 个 PaletteRole 映射(theme.rs:52-54)
      base = "#1e1e2e", mantle = "#181825", crust = "#11111b",
      surface0 = "#313244", surface1 = "#45475a", overlay = "#6c7086",
      subtext = "#a6adc8", text = "#cdd6f4",
      accent = "#cba6f7", accent_2 = "#74c7ec",
      red = "#f38ba8", yellow = "#f9e2af", green = "#a6e3a1", peach = "#fab387",
      roles = { accent = "red", muted = "subtext", faint = "overlay" },
    },
    keys = {
      -- 方向是【动作 → 键】(非 mpv/nvim 的 键→动作):与深合并语义强耦合——
      -- 用户覆盖 keys.play_pause = "x" 即干净替换;若反向(键→动作),旧键解不掉(Lua nil 无法表达删除)。
      -- 值可为单键或数组(数组整体替换);自定义动作用 ["my.skip_short"] = "S" 引用 action 注册名。
      play_pause = "space", next = "n", prev = "p",
      -- ……(全量内建动作 enum 见实现计划;现散落 app.rs:409-754)
    },
    behavior = {
      -- 交互手感(const 审计补录;接线点 = sub00 的 VolumeDelta/SeekDelta/SelectionMove)
      volume_step = 5,                     -- app.rs:33
      seek_step_secs = 5,                  -- app.rs:36
      seek_big_step_secs = 30,             -- app.rs:39
      list_jump_rows = 7,                  -- app.rs:42 与 popup/queue.rs:19(现重复定义,接线时消重)
      kill_spawned_daemon_on_exit = true,  -- runtime/daemon.rs:25;false = TUI 退出后自拉起的 daemon 续命
    },
  },
  -- 以下顶层段 = daemon/共享核心
  audio = {
    volume = 100,            -- engine.rs:50 DEFAULT_VOLUME_PCT
    backend = "auto",        -- "auto" | "null"(AudioMode 已有,engine.rs:148-161;不破坏 Null 降级)
  },
  cache = {
    audio_capacity = 10 * 1024 ^ 3,   -- 字节;Lua 可编程性示范:不需要 "10GiB" 字符串解析
    cover_capacity = 1 * 1024 ^ 3,
  },
  download = {
    quality = "lossless",    -- download.rs:35 DOWNLOAD_QUALITY
    dir = nil,               -- 缺省走 music_export_dir()(lib.rs:64-71)
  },
  sources = {
    netease = {
      timeout_secs = 100,    -- client.rs:22 TIMEOUT_SECS,需先提升为 NeteaseConfig 字段
      proxy = false,         -- false=禁用;字符串=代理 URL(NeteaseConfig 已有字段)
      max_connections = 0,   -- 0=无限(NeteaseConfig 已有字段)
    },
  },
  daemon = {
    gapless_prefetch_ms = 10000,  -- player.rs:35,"高级"档
  },
}
```

**保持内部不暴露**(2026-06-04 全仓 const 审计确认):audio tick/prefetch 字节(engine.rs:47,64)、session/heartbeat/report 间隔(player.rs:45、server.rs:132、media.rs:19)、segment_max_bytes、UA 池/base_url/crypto/登录状态码(协议常量)、FFT/kmeans/workers/prefetch RADIUS(性能与 DSP 内参,profile 驱动原则)、响应式布局比例。

**二期候选(点名暂缓,不接线)**:`FLASH_TTL`(toast 停留 4s,notifications.rs:20)、`PREV_RESTART_THRESHOLD_MS`(3s 内 prev 回上一首阈值,player.rs:38)、spectrum 观感开关(`SHOW_PEAK_CAP`/`SHOW_TRAIL`/`HUE_ROTATE`/`SPRING_*`,spectrum.rs:53-82)。

## 6. 运行时:daemon VM 与四承重墙

daemon 启动 eval config 后保留 VM(`mlua::Lua` 为 `Send`,VM + 注册表归专用脚本线程所有)。

```lua
-- ① 结构化命令(= 既有 IPC 命令的 Lua 投影,daemon 本地执行)
mineral.player.toggle() / next() / prev() / stop()
mineral.player.seek_rel(secs) / seek_to(secs)
mineral.player.set_volume(pct) / set_mode(mode) / play(song_id)
mineral.download(song_id)

-- ② 属性树:读 + 订阅(= AudioSnapshot 等领域态的投影)
mineral.get(prop)          -- "player.song" | "player.state" | "player.volume" | "player.position" | "player.mode" | "queue.length"
mineral.observe(prop, fn)  -- 语义保证(照抄 mpv):订阅即回放当前值;高频变化合并只回末值

-- ③ 离散生命周期事件(只留非属性化事故,必带 reason)
mineral.on("track_finished", fn)     -- fn(song, reason)  reason ∈ "eof"|"skip"|"error"|"stop"
                                     -- 分期:Phase 1 仅 "eof"/"skip" 可靠、"stop" best-effort;
                                     -- "error" 变体 Phase 2 落地(随 player 级播放失败信号补齐)
mineral.on("download_completed", fn) -- fn(song, path)

-- ④ 具名动作(物理键解耦;多 client 共用的触发面)
mineral.action("my.skip_short", fn)  -- fn(ctx),ctx 见 §7
mineral.bind(key, fn)                -- 语法糖 = 匿名 action + keys 表追加

-- 经 push 通道回 TUI
mineral.ui.toast(msg, { kind = "info"|"warn"|"error", id = "..." })  -- 同 id 替换不堆叠
-- 工具
mineral.log.info(msg) / warn(msg)    -- 落 mineral_log
```

**状态值变更一律走 observe,不造 `volume_changed` 类 bespoke 事件**(D7)。

## 7. 键位体系

```
TUI 收到 KeyEvent
 ├─ 视图上下文裁决(搜索输入态/全屏屏蔽等,留在 TUI——app.rs:463,474 既有逻辑)
 ├─ keys 表命中【视图动作】(滚动/全屏/浮层) → TUI 本地执行
 ├─ keys 表命中【领域动作】(播放/下载/love)  → 现成 IPC 命令(带显式参数,如选中 song_id)
 └─ keys 表命中【自定义动作名】               → KeyTriggered{ action, ctx } → daemon VM 执行 fn(ctx)
```

- 动作分两类:**内建动作**(enum,TUI/daemon 各自实现自己的子集)与**自定义动作**(daemon VM 注册)。
- 前置重构:统一 `Action` 枚举(现状无中央 action,键→动作散在 `handle_*_key()`,且主视图与浮层 `OverlayAction` 两套不统一——`popup/component.rs:79-88`)。这是键位可配的先决工程。
- `KeyContext`(`mineral-protocol` 结构体,边缘序列化):`{ view, selected_song_id, selected_playlist_id, now_playing_id }` —— 按键瞬间的只读快照,视图态本体不进 daemon。
- daemon 在 attach 握手时把"自定义动作名集合"发给 TUI;TUI 据此知道哪些键转发。

## 8. daemon→TUI push 通道

从现有雏形(下载 toast、播放快照)泛化为统一 `Event` 推送,站 nvim `ext_messages` 端(结构化语义事件,TUI 自排版;**不**学 grid 端):

- `Event::Toast { kind, content, id }` —— id 相同替换(nvim msg_show 语义)。
- `Event::PropertyChanged { prop, value }` —— observe 的协议面;**订阅即回放 + 末值合并**两条语义在 daemon 侧实现,对 Lua 与对 client 同一套。
- `Event::StoreChanged { song_id, key }` —— per-song 持久 KV(`store.*`,见 §9 P2)的**粗粒度**变更通知(MPD sticker 子系统风格)。**范围注**:observe 的属性树是**有限集**(`player.*` / `queue.*` 等可枚举的领域态),"订阅即回放 + 末值合并"语义只对这棵有限属性树成立;per-song KV 是开放命名空间的数据库、不是属性树,其变更不进 `PropertyChanged`,走独立的 `StoreChanged`(只报 song_id + key,接收方按需重读)。
- 批次原子性:一次 hook 触发的多个 UI 效果在同一 tick 应用(简化版 flush;不做跨批渲染屏障)。
- 重连全量同步:attach 握手时 daemon 下发当前属性快照 + 有效配置切片(nvim attach 重发 option_set 同款思路)。**权威顺序**:TUI 自载切片只服务首帧(D8),attach 后以 daemon 下发为准覆盖之——热重载(Phase 2)后两者才可能不同,平时二者同源同值。

## 9. Builtin API 分期目录

| 期 | API | 对应内核能力 |
|---|---|---|
| P1 | §6 四族最小集 + `ui.toast` + `log.*` | 既有 IPC 命令 / AudioSnapshot / 下载事件 |
| P2 | `queue.list/jump/add/remove`(**pos+id 双寻址**,entry_id 稳定句柄) | 队列(协议同步加 version) |
| P2 | `library.playlists/tracks/love` | 既有 channel 能力 |
| P2 | `store.get/set/inc(song_id, key)`(per-song 持久 KV,MPD stickers;**内建一等字段** local_play_count/rating/last_played;开放 key 带命名空间约定) | mineral-persist |
| P2 | `timer.after/every`(返回 timer:`stop`(保留已走)/`kill`(清零)/`resume`) | 脚本线程定时器 |
| P3 | `hook("before_play"/"before_download", fn)` 同步拦截(可改 URL/音质/跳过;**必带熔断** + `cont()/defer()`) | 多源 fallback 场景 |
| P3 | `emit(name, payload)` / `on_message(name, fn)`(自定义事件总线,MPD C2C 逃生口) | 协议旁路 |
| P3 | `spawn(args, opts, fn)` 结构化异步子进程(可中止) | tokio::process |
| P3 | `library.search(q, opts, fn)` 异步 | channel 搜索 |

## 10. 协议护栏(今天就守,远期才建)

**今天写进协议的约束(几乎零成本)**:

1. IPC 三层:**语义层**(`Request/Response/Event` 结构化 enum,serde derive)/ **codec**(bincode 今天、JSON 将来可换)/ **transport**(unix socket 今天)。
2. **request-id 配对 + Event 同连接交错下推**(规避 MPD"idle 霸占连接"之债,client 永远单连接)。
3. **Lua API 与协议同形**:observe/on/command/action 四族即协议动词。
4. 握手**结构化能力协商**:client kind/协议版本/订阅集/字段窄化(规避 MPD 弱协商)。
5. 队列 **pos+id 双寻址 + 单调 version**。
6. **二进制走旁路**(封面已独立 CoverFetcher,保持;不学 MPD `binary: N` 内联)。

**远期参考(不做,只记方向)**:Mopidy 式可插拔 frontend——nvim(shell out CLI + `mineral subscribe` ndjson push → socket JSON-RPC → toggleterm 嵌 TUI 零成本)、web(daemon 内嵌 WS frontend,loopback,出网才加 token 中间件)、MPRIS(`mineral-media` 已是第一个活证)。**MPD 协议兼容层**(`mineral-ipc-mpd` 边缘 adapter crate,把 ~30 个核心 MPD 命令映射到 daemon 的结构化 API)也列为远期 frontend 选项,可白嫖 MPD 客户端生态(M.A.L.P. / mpDris2 等)而无需自写 client;它是"边缘适配层再序列化"原则(rust 内部结构化、edge 才落 wire)的又一实例——结构化核心不变,只在 adapter 边缘做 MPD 文本协议 ↔ 结构化 Request/Event 的双向翻译。

## 11. crate 结构与注入点

| crate | 职责 | 变化 |
|---|---|---|
| `mineral-config` | schema(按域拆模块:ui/keys/audio/cache/sources/daemon,守 800 行)+ loader(mlua eval/merge/from_value)+ `lua/default.lua` + `lua/meta/*.lua` stub | 从常量箱长成;两个 `pub const` 退役 |
| `mineral-script`(新) | daemon 侧 VM 运行时:host API 绑定、hooks/actions 注册表、observe 分发、看门狗线程、定时器 | 依赖 mineral-config / -protocol |
| `mineral-protocol` | `Event` 推送、`KeyTriggered`/`KeyContext`、握手 ClientInfo/能力协商、request-id(对齐 §10,如已有则核对) | 增量 |
| `mineral-server` | 接 `mineral-script`;config 注入 MediaCache(`media_cache.rs:251`)/download(`download.rs:35`)/player(`player.rs:35`)/engine(volume `engine.rs:50`、AudioMode) | 注入 |
| `mineral-tui` | `Theme::from_config`(替换硬编码 `Theme::default()` 路径,App 持 `Arc<Theme>`)、声明式 keymap 解析、Action 枚举统一、Event 消费 | 注入 + 前置重构 |
| `mineral` / `mineral-cli` | 启动链 `Args::parse()` 后、`open_persist()`(`main.rs:74`)/`build_channels()`(`main.rs:140`)前加载 config;`mineral config init` / `mineral config check` 子命令 | 注入 |

mlua 依赖面:mineral-config(loader)被 TUI/CLI/server 公用——三者本就要 eval;vendored 编译成本一次性。

## 12. 错误处理与降级(对齐项目语义)

- 解析/类型错误:`serde_path_to_error` 风格带字段路径;日志 `error = mineral_log::chain(&e)`;TUI toast 展示 file:line + 路径;**继续以默认配置启动**。
- hook 运行错误:pcall 捕获 → toast(带 hook 名 + Lua traceback 首行)→ 该次失败不致命,hook 不自动禁用(连续失败 N 次告警,熔断策略实现期定)。
- `backend = "null"` 等配置值**不得破坏既有降级链**(audio 无设备降 Null、cover fetcher 降 disabled、netease 未登录返 `Ok(None)`)。

## 13. 看门狗

- hooks/actions 在 `mineral-script` 专用线程执行(VM 归该线程,daemon 主循环经 channel 投递事件/命令,**不直接持锁**)。
- Lua 指令计数 hook(mlua `set_hook`)+ 墙钟双阈值:超软阈值 warn(log+toast),超硬阈值中断该次执行。阈值进 `daemon` 配置段(有默认)。
- P3 的同步拦截 hook 另有 `defer()/cont()`,超时未 cont 视为放行 + warn。

## 14. 测试策略(对齐 docs/testing.md)

- **default.lua 守卫**:eval + `from_value::<Config>` 必须成功(防字段漂移,核心测试)。
- merge 语义单测:partial 覆盖 / 嵌套深合并 / 数组整体替换 / 用户文件缺失;proptest 不变量(`merge(d, {}) == d` 等)。
- 错误路径:坏 Lua / 类型错 → 落默认 + 错误含字段路径(assert on `chain` 文本)。
- keymap:`test_support::app_with_queue` + 真实 `KeyEvent` 跨 tick,重映射后动作生效;快照测 `config check` 输出(`assert_snap!` 带中文 description)。
- hooks e2e:daemon e2e 串行组(`.config/nextest.toml` 既有)+ `MINERAL_AUDIO_NULL=1`;observe 两条语义(回放/合并)单测。
- 真实 IO/engine 测试 `multi_thread` rt(memory 既有教训)。

## 15. 分期交付

| Phase | 内容 | 交付判据 |
|---|---|---|
| **0 纯声明** | loader + default.lua(LuaCATS)+ 强类型 Config + 深合并;接通:主题 14 token + roles、键位重映射(含 Action 枚举统一前置重构)、音量/后端、缓存容量、netease timeout/proxy/max_connections、下载质量;`config init/check`;const 退役 | 不写配置行为不变;写配置各旋钮生效;default.lua 守卫测试绿 |
| **1 daemon VM** | mineral-script crate;四承重墙最小集;push 通道泛化(Toast/PropertyChanged);看门狗;observe 语义 | `on("track_finished")`+`observe("player.volume")`+`ui.toast` e2e 绿;headless 跑 hook |
| **2 动作与生态** | action/bind + KeyTriggered/KeyContext;queue/library/store/timer API;热重载(daemon watch + push 增量) | 自定义动作绑键可用;store 持久可见 |
| **3 强力位** | before_play/before_download 拦截(熔断)、emit/on_message、spawn、search | 多源 fallback 脚本可写 |

## 16. 风险与缓解

| 风险 | 缓解 |
|---|---|
| 慢/死循环 hook 卡 daemon | §13 专线程 + 双阈值熔断;nvim 同款痛点已预设防 |
| default.lua 与 Config 漂移 | §4 守卫测试 + deny_unknown_fields |
| 配置顶层副作用多次执行(D8 多进程 eval) | 文档明示"顶层只做纯计算";hooks 注册在非 daemon 进程为 no-op |
| mlua(C)进编译链 | vendored 一次性;CI 缓存;`libasound2-dev` 先例在 |
| Action 枚举统一重构面大(app.rs 600+ 行键处理) | Phase 0 内独立 PR 先行,行为快照测试护航 |
| Lua `nil` 无法表达显式置空 | schema 逐字段用 `false`/枚举表达关闭语义(§3) |
