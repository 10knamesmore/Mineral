# sub02:声明旋钮全线接线(Phase 0 注入)

> 母设计:[`2026-06-04-user-config-lua-design.md`](./2026-06-04-user-config-lua-design.md)(已定宪法,本文只细化落地)。
> 本文档按项目约定不纳入版本控制。
> 状态:实现级 subspec,open questions 已裁决(见 §10),可进实现计划。

## 0. 一句话

把 sub01 落成的强类型 `Config` 从 `mineral` / `mineral-cli` 的启动链注入到所有声明旋钮的消费点:主题 14 token + 3 roles、初始音量、音频后端、缓存容量(audio/cover)、下载音质、netease(timeout/proxy/max_connections)、gapless 预排窗口、behavior 交互手感 5 旋钮(volume_step / seek_step_secs / seek_big_step_secs / list_jump_rows / kill_spawned_daemon_on_exit);并退役 `mineral-config` 的两个 `pub const` 及 behavior 对应的 5 个 const。**不触及** Lua VM / hooks / 键位解析执行(那是 sub00/Phase 1+)。

## 1. 范围与不做

### 做(本 subspec 负责)

- 在 `mineral`(`main.rs`)与 `mineral-cli`(`serve.rs` / `cache/command.rs`)启动链里,把 sub01 产出的 `Config` 实例传到各消费点。
- 改造各消费点的构造签名,让它们从硬编码常量改吃 `Config` 切片(或切片派生的具体参数)。
- `Theme::from_config(&ThemeConfig)`,`App` 持 `Arc<Theme>`(替换 `Theme::default()` 硬写)。
- `NeteaseConfig` 增 `timeout_secs` 字段(常量 `TIMEOUT_SECS` 提升为字段);`NeteaseConfig` 顺势改造为合规配置 struct(私有字段 + `#[non_exhaustive]` + builder + getters)。
- 退役 `mineral-config::{AUDIO_CACHE_CAPACITY, COVER_CACHE_CAPACITY}` 两个 `pub const`(`lib.rs:7,10`),改由 `Config` 提供。
- **behavior 交互手感 5 旋钮接线**(const 审计补录):`volume_step`(`app.rs:33` VOLUME_STEP)、`seek_step_secs`(`app.rs:36` SEEK_STEP_S)、`seek_big_step_secs`(`app.rs:39` SEEK_BIG_STEP_S)、`list_jump_rows`(`app.rs:42` ROW_BIG_STEP,**并消除 `popup/queue.rs:19` 的重复 `ROW_BIG_STEP` 定义**——两处统一从注入的 keymap/behavior 取)、`kill_spawned_daemon_on_exit`(`runtime/daemon.rs:25` KILL_SPAWNED_DAEMON_ON_EXIT)。前四个经 sub00 的 Action 参数(`VolumeDelta`/`SeekDelta`/`SelectionMove`)灌入,App/Keymap 构造时从 `cfg.tui().behavior()` 读;daemon 续命开关在 TUI 退出路径(`runtime/daemon.rs`)读。退役这 5 个 const。
- CLI flag / `MINERAL_*` env / config.lua / default.lua 四级优先级的**落地位置**(谁覆盖谁,在哪个边缘层 resolve)。
- 论证不破坏既有降级链;逐旋钮生效测试 + 不写配置行为不变测试。

### 不做(边界,留给相邻 subspec)

- **sub01**:`Config` / `ThemeConfig` / `AudioConfig` 等子结构的定义、serde、`deny_unknown_fields`、`default.lua`、Lua 加载/合并/`from_value`、`config init`/`config check` 子命令。本文只**消费** `Config` 的 getters,不定义它。
- **sub00**:`keys` 表语义、`Action` 枚举统一、`handle_*_key` 重构、keymap 解析与转发。本文不接键位执行;keys 切片(主 spec D12:经 `cfg.tui().keys()` 取,已收进 client 命名空间)由 sub00 消费。
- **Phase 1+**:daemon VM、`mineral-script`、四承重墙、observe、push 通道泛化、看门狗、热重载。
- **不暴露的内部参数**(母设计 §5 末):audio TICK/PREFETCH_BYTES、session/heartbeat 间隔、segment_max_bytes、UA 池、base_url、`PLAYBACK_QUALITY`(播放音质本期**不**进配置面,仅下载音质进)。
- `download.dir` 旋钮:**已裁决本期接线**(Q4)——在 `open_env` 处用 config 值覆盖 `music_export_dir()` 默认;优先级 **env(`MINERAL_DOWNLOAD_DIR`)> config**。

## 2. 现状锚点(file:line,已核对)

### 主题

- `crates/mineral-tui/src/render/theme.rs:15-44` —— `Theme` 14 个 `Color` token(`pub` 字段,`#[derive(Clone, Copy)]`)。
- `theme.rs:50-56` —— `source_color(&self, role: PaletteRole)` 把 3 个 `PaletteRole`(Accent/Muted/Faint)映射到 `red/subtext/overlay`(硬编码 role→token)。
- `theme.rs:59-76` —— `mocha_mauve()` 是 `const fn`,14 个 `Color::Rgb(...)` 字面量。
- `theme.rs:79-83` —— `impl Default`,返回 `mocha_mauve()`。
- `crates/mineral-tui/src/app.rs:56` —— `pub theme: Theme`(**按值持有**,`Theme: Copy`)。
- `app.rs:120` —— `App::new` 里 `theme: Theme::default()` 硬写。
- 全仓 `&Theme::default()` 调用点均在 `#[cfg(test)]` 内(popup/queue.rs 等),非生产路径。

### 音频

- `crates/mineral-audio/src/engine.rs:50` —— `const DEFAULT_VOLUME_PCT: u8 = 100`(注意:母设计写的 50 是**笔误**,代码实际 100)。
- `engine.rs:171` —— `player.set_volume(pct_to_gain(DEFAULT_VOLUME_PCT))`(引擎线程内初始化音量)。
- `crates/mineral-audio/src/handle.rs:25-32` —— `AudioMode { Auto, ForceNull }`。
- `handle.rs:85` —— `AudioHandle::spawn(mode: AudioMode) -> Result<(Self, SpectrumTap)>`。
- `handle.rs:87-90` —— spawn 里 `AudioSnapshot { volume_pct: 100, .. }` 硬写初始快照音量。
- `handle.rs:205-207` —— `set_volume(&self, pct: u8)`,clamp `pct.min(100)`。
- `crates/mineral-audio/src/lib.rs:14-15` —— 导出 `AudioHandle/AudioMode/SpectrumTap/AudioBackend/AudioSnapshot`。

### server

- `crates/mineral-server/src/server.rs:13` —— `use mineral_config::AUDIO_CACHE_CAPACITY`。
- `server.rs:42-46` —— `Server::spawn(channels, audio_mode: AudioMode, persist: ServerStore)`。
- `server.rs:48` —— `AudioHandle::spawn(audio_mode)?`;`server.rs:51` —— `PlayerCore::spawn(audio, scheduler, channels, persist, media_cache)`。
- `server.rs:96-114` —— `open_media_cache(persist)`:内部 `MediaCache::open(persist, dir, AUDIO_CACHE_CAPACITY)`(`server.rs:104`)。
- `crates/mineral-server/src/media_cache.rs:35-38` —— `MediaCache::open(persist, dir, capacity: u64)`;`:53` `disabled()`。
- `crates/mineral-server/src/download.rs:35` —— `const DOWNLOAD_QUALITY: BitRate = BitRate::Lossless`;唯一用点 `download.rs:322`(worker 调 `download_song(.., DOWNLOAD_QUALITY, ..)`)。
- `download.rs:42-55` —— `open_env() -> (Option<reqwest::Client>, Option<PathBuf>)`(music_dir 走 `music_export_dir()`)。
- `crates/mineral-server/src/player.rs:31` —— `const PLAYBACK_QUALITY: BitRate = BitRate::Exhigh`(**本期不进配置**)。
- `player.rs:35` —— `const PREFETCH_LEAD_MS: u64 = 10_000`(= `daemon.gapless_prefetch_ms`);用点 `gapless.rs:102`。
- `player.rs:109-142` —— `PlayerCore::spawn(audio, scheduler, channels, persist, media_cache)`;内部 `open_env()`(`player.rs:116`)。
- `crates/mineral-server/src/lib.rs:33` —— `pub use mineral_audio::AudioMode`。

### cover / TUI 启动

- `crates/mineral-tui/src/runtime/cover_fetch.rs:171` —— `store.cover_cache(dir, mineral_config::COVER_CACHE_CAPACITY)`。
- `crates/mineral-tui/src/lib.rs:54-101` —— `pub async fn run(channels, launch: Launch, persist)`;`:88` in-proc `Server::spawn(channels, AudioMode::Auto, persist)`。
- `lib.rs:105-125` —— `run_app(client, cover_fetcher)`;`:115` `App::new(client, cover_fetcher, cover_encoder, picker, anchor)`。

### behavior 交互手感(const 审计补录)

- `crates/mineral-tui/src/app.rs:33` —— `const VOLUME_STEP: i16 = 5`;用点 `app.rs:573-574`(`nudge_volume(±VOLUME_STEP)`)。
- `app.rs:36` —— `const SEEK_STEP_S: i64 = 5`;用点 `app.rs:575-576`(`seek_relative(±SEEK_STEP_S)`)。
- `app.rs:39` —— `const SEEK_BIG_STEP_S: i64 = 30`;用点 `app.rs:560,564`。
- `app.rs:42` —— `const ROW_BIG_STEP: usize = 7`;用点 `app.rs:640,644,704,707`(列表大步跳行)。
- `crates/mineral-tui/src/components/popup/queue.rs:19` —— `const ROW_BIG_STEP: usize = 7`(**与 `app.rs:42` 重复定义**;用点 `:127,131`);本期统一从注入的 behavior/keymap 取,消重。
- `crates/mineral-tui/src/runtime/daemon.rs:25` —— `const KILL_SPAWNED_DAEMON_ON_EXIT: bool = true`;用点 `:134`(退出路径 `if !KILL_SPAWNED_DAEMON_ON_EXIT { return }`,模块注释 `:5`/`:121`/`:128` 也引此名)。

### CLI / 启动链

- `crates/mineral/src/main.rs:15-29` —— `main()`:`color_eyre::install()` → `mineral_log::init()` → `Args::parse()` → 分发。
- `main.rs:57-68` —— `serve_blocking()`:`open_persist().await` → `build_channels(persist)` → `serve_run(channels, persist)`。
- `main.rs:74-106` —— `open_persist()`(降级 disabled)。
- `main.rs:122-130` —— `run_tui`:in-proc `build_channels`,然后 `mineral_tui::run(channels, launch, persist)`。
- `main.rs:140-156` —— `build_channels(persist)`;`:162-177` `build_netease(persist)`:`NeteaseChannel::with_credential(&NeteaseConfig::default(), ..)`(`main.rs:169`)。
- `crates/mineral-cli/src/subcommands/serve.rs:26-83` —— `serve_run(channels, persist)`:`:47` 读 `MINERAL_AUDIO_NULL` env 决定 `AudioMode`,`:52` `Server::spawn(channels, audio_mode, persist)`。
- `crates/mineral-cli/src/subcommands/cache/command.rs:8` —— `use mineral_config::{AUDIO_CACHE_CAPACITY, COVER_CACHE_CAPACITY}`;`:56,64` 用于 `audio_cache(..)` / `cover_cache(..)`。
- `crates/mineral-cli/src/core.rs:19-34` —— `Args { command, connect, in_proc }`(clap)。

### netease

- `crates/mineral-channel/netease/src/config.rs:2-8` —— `NeteaseConfig { max_connections: usize, proxy: Option<String> }`(**公共字段**,违反对外配置 struct 约定,本期顺手修)。
- `crates/mineral-channel/netease/src/transport/client.rs:22` —— `const TIMEOUT_SECS: u64 = 100`;用点 `client.rs:51,66`(`Transport::new` / `from_cookie_jar` 各一次 `.timeout(Duration::from_secs(TIMEOUT_SECS))`)。
- `client.rs:49,64` —— `Transport::new(config: &NeteaseConfig)` / `from_cookie_jar(config, jar)`,读 `config.max_connections` / `config.proxy`。
- `crates/mineral-channel/netease/src/channel.rs:40,59,74,90` —— `new` / `with_cookie` / `with_credential` / `build` 全部 `config: &NeteaseConfig`。
- `crates/mineral-channel/netease/src/cli.rs:59` —— `NeteaseConfig::default()`(qr 登录路径)。
- `channel.rs:371`(test)—— `NeteaseConfig::default()`。

## 3. 新增 / 修改文件清单

> 本 subspec **不新增 crate**,只改既有文件;`Config` 类型本身由 sub01 在 `mineral-config` 提供。

| 文件 | 改动 | 规模 |
|---|---|---|
| `crates/mineral-tui/src/render/theme.rs` | 增 `Theme::from_config(&ThemeConfig) -> Theme`;`source_color` 改读 roles 配置(role→token 不再硬编码) | +40~60 行,仍 < 200 |
| `crates/mineral-tui/src/app.rs` | `theme: Theme` → `theme: Arc<Theme>`;`App::new` 增 `theme: Arc<Theme>` 参数,删 `Theme::default()`;删 `VOLUME_STEP`/`SEEK_STEP_S`/`SEEK_BIG_STEP_S`/`ROW_BIG_STEP` 4 const,改从注入的 keymap/behavior 取(经 sub00 的 `VolumeDelta`/`SeekDelta`/`SelectionMove` Action 参数灌入,App/Keymap 构造时读 `cfg.tui().behavior()`) | 改 ~15 行;签名 +1 参数 |
| `crates/mineral-tui/src/lib.rs` | `run` 增 `config: Config` 参数;`run_app` 透传 `Arc<Theme>`;in-proc `Server::spawn` 传 config 派生的 audio/cache 切片;cover_fetcher 容量从 config 取 | 改 ~20 行 |
| `crates/mineral-tui/src/runtime/cover_fetch.rs` | `cover_cache(dir, capacity)` 的 capacity 改由 `CoverFetcher::spawn` 入参传入(取代 `mineral_config::COVER_CACHE_CAPACITY`) | 改 ~10 行 |
| `crates/mineral-tui/src/components/popup/queue.rs` | **删重复 `const ROW_BIG_STEP`(`:19`)**;`:127,131` 的大步跳行改用注入的 `list_jump_rows`(经 `SelectionMove` Action 参数 / keymap 传入,不再本地 const)。与 `app.rs:42` 统一单一真相源 | 改 ~6 行 |
| `crates/mineral-tui/src/runtime/daemon.rs` | 删 `const KILL_SPAWNED_DAEMON_ON_EXIT`(`:25`);退出路径(`:134`)改读注入的 `cfg.tui().behavior().kill_spawned_daemon_on_exit()`;更新引此名的模块注释(`:5`/`:121`/`:128`) | 改 ~8 行 |
| `crates/mineral-audio/src/handle.rs` | `AudioHandle::spawn` 增 `initial_volume: u8` 参数;初始 `AudioSnapshot.volume_pct` 用它;透传给 `engine::run` | 改 ~10 行 |
| `crates/mineral-audio/src/engine.rs` | 删 `DEFAULT_VOLUME_PCT`;`run`/`engine_main` 增 `initial_volume: u8`;`player.set_volume(pct_to_gain(initial_volume))` | 改 ~15 行 |
| `crates/mineral-server/src/server.rs` | `Server::spawn` 增 `ServerConfig`(audio_volume/cache_capacity/download_quality/prefetch_ms 等切片);删 `AUDIO_CACHE_CAPACITY` import;`open_media_cache` 增 capacity 参数 | 改 ~25 行 |
| `crates/mineral-server/src/player.rs` | `PlayerCore::spawn` 增 download_quality / prefetch_ms;`PREFETCH_LEAD_MS` 退化为默认值来源或删;存 `download_quality` 进 `Inner` | 改 ~20 行 |
| `crates/mineral-server/src/download.rs` | 删 `const DOWNLOAD_QUALITY`;worker 读 `player.download_quality()` | 改 ~6 行 |
| `crates/mineral-server/src/gapless.rs` | `PREFETCH_LEAD_MS` 用点改读 `player.prefetch_lead_ms()` | 改 ~4 行 |
| `crates/mineral-channel/netease/src/config.rs` | 加 `timeout_secs` 字段;改造为私有字段 + `#[non_exhaustive]` + `typed-builder` + `derive-getters`;留 `Default` | 重写,~40 行 |
| `crates/mineral-channel/netease/src/transport/client.rs` | 删 `const TIMEOUT_SECS`;`Transport::new`/`from_cookie_jar` 读 `config.timeout_secs()` | 改 ~6 行 |
| `crates/mineral-channel/netease/src/channel.rs` | 适配 getter 访问(若改私有字段);test/cli 用 builder | 改 ~5 行 |
| `crates/mineral/src/main.rs` | 启动链顶加载 `Config`(sub01 loader);`serve_blocking`/`run_tui`/`build_netease` 透传 config 切片 | 改 ~25 行 |
| `crates/mineral-cli/src/subcommands/serve.rs` | `serve_run` 增 `config` 参数;env→config 优先级在此 resolve(`MINERAL_AUDIO_NULL` 优先于 config.audio.backend) | 改 ~15 行 |
| `crates/mineral-cli/src/subcommands/cache/command.rs` | 删 const import;capacity 从 `Config`(自 eval)取 | 改 ~8 行 |
| `crates/mineral-config/src/lib.rs` | 删两个 `pub const`(sub01 已把容量挪进 `CacheConfig`) | -4 行(由 sub01 主导,本文协调退役顺序) |

**守约**:所有改动文件均 < 800 行;`lib.rs` / `mod.rs` 不写逻辑(`mineral-tui/src/lib.rs::run` 已是薄分发,新增 config 透传仍是布线非逻辑,可接受;真正的 `Theme::from_config` 逻辑落 `theme.rs`)。

## 4. 关键类型与签名

> `ThemeConfig` / `BehaviorConfig` / `AudioConfig` / `CacheConfig` / `DownloadConfig` / `SourcesConfig`(内含 `NeteaseSection`)/ `DaemonConfig` 等子结构由 **sub01** 定义(私有字段 + `#[non_exhaustive]` + getters)。下方签名假定其 getters 形如 `cfg.audio().volume()` / `cfg.tui().behavior().volume_step()`。本文只声明**消费侧**新增/改造的签名。

### Theme::from_config(theme.rs)

```rust
/// 从配置切片构造主题:14 token 各取一个 `Color`,3 个 [`PaletteRole`] 角色
/// 各映射到一个 token 名。配置非法值(色值解析失败)由 sub01 在 `from_value`
/// 阶段拒绝并落默认,到这里的 `ThemeConfig` 已是合法强类型。
///
/// # Params:
///   - `cfg`: 主题配置切片(sub01 的 `ThemeConfig`,getters 取 token / roles)
///
/// # Return:
///   落地后的 [`Theme`]。
pub fn from_config(cfg: &mineral_config::ThemeConfig) -> Self {
    // 14 token 逐个由 cfg.base() 等 getter 取(sub01 已解析成 ratatui Color)。
    // roles:cfg.roles().accent() 返回 token 名(枚举),此处 resolve 到具体 Color
    // 并随实例存(取代 source_color 里的硬编码 match)。
    todo!("由实现期落地;不在本 spec 内写实现")
}
```

`source_color` 改造:roles 从配置来,需把 resolve 后的 3 个 `Color` 存进 `Theme`(新增 3 个私有字段 `role_accent / role_muted / role_faint`),`source_color` 改读字段而非 `match` 硬编码。`Theme` 保持 `Copy`(全字段 `Color: Copy`)。

### App 持 Arc\<Theme\>(app.rs)

```rust
/// 应用顶层状态。
pub struct App {
    /// 当前主题(`Arc` 共享:future 热重载时整体换,渲染处只读引用)。
    pub theme: std::sync::Arc<Theme>,
    // ……其余字段不变
}

impl App {
    /// 构造 App。
    ///
    /// # Params:
    ///   - `theme`: 已由配置落地的主题(`Arc` 共享只读)
    ///   - ……(其余 params 不变)
    pub fn new(
        client: std::sync::Arc<dyn Client>,
        cover_fetcher: CoverFetcher,
        cover_encoder: CoverEncoder,
        picker: Picker,
        launch_anchor: Option<Position>,
        theme: std::sync::Arc<Theme>,
    ) -> Self { /* theme 直接存,删 Theme::default() */ }
}
```

> 渲染处签名 `fn render_content(.., theme: &Theme, ..)` 不变:`&*app.theme` 解引用即得 `&Theme`。`Theme: Copy` 仍可按值传给少数需要 owned 的处(代价是一次 16 字段 copy,可忽略)。

### behavior 5 旋钮接线点

`cfg.tui().behavior()` 产出 `&BehaviorConfig`(sub01 定义),5 个 getter 各对应一个原 const。接线分两类落点:

- **前四个经 sub00 的 Action 参数灌入**(键事件 → Action 时携带步长,App 不再读裸 const):
  - `volume_step()` → `VolumeDelta`(`±volume_step`;原 `app.rs:33` `VOLUME_STEP`,类型 `i16`,getter 返 `u8` 在构造 delta 时 `i16::from`)。
  - `seek_step_secs()` / `seek_big_step_secs()` → `SeekDelta`(原 `app.rs:36,39` `SEEK_STEP_S`/`SEEK_BIG_STEP_S`,类型 `i64`,getter 返 `u32` 经 `i64::from`)。
  - `list_jump_rows()` → `SelectionMove`(大步;原 `app.rs:42` `ROW_BIG_STEP` 与 `popup/queue.rs:19` 重复定义,**两处统一从注入的 keymap/behavior 取**,getter 返 `u16` 经 `usize::from`)。
  - 落地:`App` / `Keymap` 构造时(`App::new` 或其依赖的 keymap 注入点)从 `cfg.tui().behavior()` 读出这四个值存入,键处理生成 Action 时填入对应 delta。具体 Action 形状由 **sub00** 定;本文只约定值来源 = behavior getter,**不**自定义 Action(避免与 sub00 双源)。
- **`kill_spawned_daemon_on_exit()` 在 TUI 退出路径读**(非 Action):`runtime/daemon.rs` 的退出钩子(`:134` 原 `if !KILL_SPAWNED_DAEMON_ON_EXIT`)改读注入的 behavior 值;daemon 句柄持有者构造时从 `cfg.tui().behavior()` 取。

> behavior 各 getter 返回类型由 sub01 定(`volume_step: u8` / `seek_step_secs`/`seek_big_step_secs`: `u32` / `list_jump_rows`: `u16` / `kill_spawned_daemon_on_exit`: `bool`),消费侧按需 `From` 拓宽到原 const 的有符号/`usize` 类型(无 `as`)。

### AudioHandle::spawn / engine::run(handle.rs / engine.rs)

```rust
/// 启动 engine 线程并返回 (handle, spectrum tap)。
///
/// # Params:
///   - `mode`: 后端选择(无设备时 `Auto` 降级 null)
///   - `initial_volume`: 初始音量百分比(0..=100,内部 clamp);来自配置 `audio.volume`
pub fn spawn(
    mode: AudioMode,
    initial_volume: u8,
) -> color_eyre::Result<(Self, SpectrumTap)> { /* snapshot.volume_pct = initial_volume.min(100) */ }
```

`engine::run` / `engine_main` 末尾增 `initial_volume: u8` 参数(取代 `DEFAULT_VOLUME_PCT`),`player.set_volume(pct_to_gain(initial_volume))`。

### Server::spawn(server.rs)—— 引入 ServerConfig 聚合切片

为避免 `Server::spawn` 参数爆炸,**新增一个 server 本地的强类型聚合**(对外配置 struct 约定:私有字段 + `#[non_exhaustive]` + getters;构造方是 `mineral` binary,用 builder):

```rust
/// daemon 启动所需的配置切片(从全局 `Config` 派生)。
///
/// 私有字段 + builder 构造 + getters,符合项目对外配置 struct 约定。
#[non_exhaustive]
#[derive(Clone, Debug, typed_builder::TypedBuilder, derive_getters::Getters)]
pub struct ServerConfig {
    /// 初始音量百分比(0..=100)。
    initial_volume: u8,

    /// 音频本体缓存容量上限(字节)。
    audio_cache_capacity: u64,

    /// 下载音质。
    download_quality: mineral_model::BitRate,

    /// gapless 预排触发距曲终的剩余窗口(毫秒)。
    gapless_prefetch_ms: u64,
}

impl Server {
    /// # Params:
    ///   - `audio_mode`: 后端选择(env / config resolve 后的最终值)
    ///   - `config`: daemon 配置切片
    pub async fn spawn(
        channels: Vec<std::sync::Arc<dyn MusicChannel>>,
        audio_mode: AudioMode,
        persist: ServerStore,
        config: ServerConfig,
    ) -> color_eyre::Result<Self> { /* AudioHandle::spawn(audio_mode, *config.initial_volume())? 等 */ }
}
```

> `audio_mode` 仍独立传(它经 env 短路,见 §5),不进 `ServerConfig`;或把 backend 也纳入 `ServerConfig` 由 caller 在 resolve 后塞入 —— 见 open questions Q2。

### NeteaseConfig 改造(config.rs)

```rust
/// `NeteaseChannel` 的构造参数。
#[non_exhaustive]
#[derive(Clone, Debug, typed_builder::TypedBuilder, derive_getters::Getters)]
pub struct NeteaseConfig {
    /// 最大并发连接数(`0` = 不限)。
    #[builder(default = 0)]
    max_connections: usize,

    /// 代理地址(`None` = 不走代理),如 `socks5://127.0.0.1:1080`。
    #[builder(default, setter(strip_option))]
    proxy: Option<String>,

    /// 单次请求超时(秒)。原 `transport::client::TIMEOUT_SECS` 常量提升至此。
    #[builder(default = 100)]
    timeout_secs: u64,
}

impl Default for NeteaseConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}
```

`Transport::new` / `from_cookie_jar` 改 `.timeout(Duration::from_secs(*config.timeout_secs()))`,`config.max_connections()` / `config.proxy()` 同理(getter 返 `&`,`*` 解 `usize`,`.as_deref()` 取 `proxy`)。

### CLI / TUI run 透传

```rust
// mineral-cli serve.rs
pub async fn run(
    channels: Vec<std::sync::Arc<dyn MusicChannel>>,
    persist: ServerStore,
    config: mineral_config::Config,
) -> color_eyre::Result<()> { /* env > config.audio.backend 在此 resolve audio_mode */ }

// mineral-tui lib.rs
pub async fn run(
    channels: Vec<std::sync::Arc<dyn MusicChannel>>,
    launch: Launch,
    persist: ServerStore,
    config: mineral_config::Config,
) -> color_eyre::Result<()> { /* Theme::from_config(config.tui().theme()) → Arc;in-proc 派生 ServerConfig */ }
```

## 5. 优先级与降级链(论证)

### 四级优先级落地位置:CLI flag > MINERAL_* env > config.lua > default.lua

- **config.lua > default.lua**:在 **sub01 的 loader 内**完成(Lua 深合并),本 subspec 拿到的 `Config` 已是合并后产物,不重复处理。
- **env > config**:在 **binary 边缘**(`serve.rs` / 未来 `main.rs`)resolve,**不进 lib**(遵守「env 只在 binary 边缘读」既有约定,见 serve.rs:47 注释)。
  - `MINERAL_AUDIO_NULL`:`serve.rs` 现有逻辑保留——`var_os().is_some()` 时 `AudioMode::ForceNull`,**否则**才看 `config.audio().backend()`(`"null"` → ForceNull,`"auto"` → Auto)。即 env 命中短路 config。
  - `MINERAL_SOCKET_DIR`:由 `mineral-paths` 既有逻辑读,本期**不**让 config 覆盖(保持兼容)。
  - `MINERAL_DOWNLOAD_DIR`:**已裁决接线 `download.dir`(Q4)**——优先级 **env > config > `music_export_dir()` 默认**;env 命中短路 config,在 `open_env` 边缘 resolve(`download.rs:42`)。
- **CLI flag > env**:Phase 0 暂**无**新增声明旋钮对应的 CLI flag(`--connect`/`--in_proc` 不是旋钮)。优先级链留作架构占位,真有 flag(如未来 `--volume`)时在 `Args::parse()` 后、`run`/`serve_run` 调用前 resolve 覆盖。**本期只写位置约定,不引入实际 flag**(见 Q3)。

### 不破坏降级链(逐条论证)

1. **audio null**:`backend = "null"`(config)或 `MINERAL_AUDIO_NULL`(env)→ `AudioMode::ForceNull`,与现有 `engine.rs:148-166` 降级**同一路径**(`sink = None` → `run_null_mode`)。`initial_volume` 注入只改 snapshot 初值与 `set_volume`,null 模式下 `set_volume` 由 handle 直写 snapshot(`handle.rs:246-247` 测试已证),**不依赖 sink**。结论:config 注入不新增 null 模式失败面。
2. **cover disabled**:`CoverFetcher::spawn` 内部 `open_cover_cache` 任一步失败仍 `return None` → 降级不缓存(`cover_fetch.rs:150-179`)。capacity 从常量改为入参,只是**传值变化**,不改 `Result` 流;`CoverFetcher::disabled()`(`:187`)路径完全不碰 capacity。结论:不受影响。
3. **netease 未登录 Ok(None)**:`build_netease`(`main.rs:162-177`)的 `load_stored()? else return Ok(None)` 早返回在构造 `NeteaseConfig` **之前**;config 注入只改 `NeteaseConfig` 的字段来源(`Default` → builder + config 值),不改早返回逻辑。`NeteaseConfig::default()` 仍可用(builder 全默认),`with_credential` 签名不变(仍收 `&NeteaseConfig`)。结论:不受影响。
4. **persist disabled** / **media_cache disabled**:capacity 注入只在 `MediaCache::open` 成功路径用;`open_media_cache`(`server.rs:96`)的目录解析失败 / open 失败仍降级 `MediaCache::disabled()`。capacity 是入参,不改这两个降级分支。

## 6. 实现步骤(依赖顺序,可拆 PR)

> 前置:sub01 已落 `Config` + 子结构 getters + loader(本 subspec 所有步骤依赖 `mineral_config::Config` 可构造)。

**PR-A:叶子常量退役 + netease config 改造(无启动链改动,可独立合并)**
1. `NeteaseConfig` 改私有字段 + builder + getters + `timeout_secs`;`client.rs` 删 `TIMEOUT_SECS` 读 getter;`channel.rs`/`cli.rs`/test 适配。跑 `cargo nextest run -p mineral-channel-netease`。
2. `mineral-config` 删两个 `pub const`(与 sub01 协调:sub01 的 `CacheConfig` 必须先就位)。**退役顺序**:先让 sub01 提供 `Config::default().cache().audio_capacity()` 等价值 → 各消费点改读 config → **最后**删 const(否则中间态编译断)。可在 PR-A 内分两 commit:commit1 加 config 路径并保留 const、commit2 删 const。

**PR-B:audio 初始音量参数化**
3. `engine.rs` 删 `DEFAULT_VOLUME_PCT`,`run`/`engine_main` 加 `initial_volume`;`handle.rs::spawn` 加 `initial_volume`,初始 snapshot 用它。
4. 改 `AudioHandle::spawn` 的所有调用点:`server.rs:48`(经 `ServerConfig`)、`handle.rs` test(`:239` 传具体值)。跑 `cargo nextest run -p mineral-audio`(real engine test 须 `multi_thread`,见 §7)。

**PR-C:server 配置切片注入**
5. 定义 `ServerConfig`(builder + getters);`Server::spawn` 加参数;`open_media_cache` 加 capacity;`PlayerCore::spawn` 加 download_quality / prefetch_ms 并存 `Inner`;`download.rs` worker 读 getter;`gapless.rs` 读 getter;删 `DOWNLOAD_QUALITY` / 退化 `PREFETCH_LEAD_MS`。
6. 改 `Server::spawn` 调用点:`serve.rs:52`、`mineral-tui/src/lib.rs:88`。

**PR-D:theme 配置化**
7. `Theme::from_config` + roles 字段化 + `source_color` 改读字段。
8. `app.rs`:`theme: Arc<Theme>`,`App::new` 加参;`lib.rs::run_app` 透传。

**PR-E:启动链接线(收口)**
9. `main.rs`:启动顶部 `let config = mineral_config::load()?`(sub01 入口,降级落默认);`serve_blocking` → `serve_run(channels, persist, config)`;`run_tui` → `mineral_tui::run(channels, launch, persist, config)`;`build_netease` 用 config 派生 `NeteaseConfig`。
10. `serve.rs`:`run` 加 config 参,env > config resolve `audio_mode`,派生 `ServerConfig` 传 `Server::spawn`。
11. `cover_fetch.rs`:`CoverFetcher::spawn(cover_capacity)` 加参,`lib.rs::run` 从 config 取传入。
12. `cache/command.rs`:从 `Config`(自 eval,CLI 离线)取 capacity。

> PR-A/B/C/D 内部尽量保持「加新参数、默认值 = 旧常量」使每步可独立绿;PR-E 才真正把 config 值灌入。

## 7. 测试清单(对齐 docs/testing.md)

- **不写配置行为不变(核心防回归)**:`Config::default()`(纯 default.lua,sub01 保证可构造)派生出的 `Theme` / `ServerConfig` / `NeteaseConfig` 各字段 `assert_eq!` 等于旧硬编码常量值:
  - `Theme::from_config(Config::default().tui().theme())` 逐 token `assert_eq!` `Theme::mocha_mauve()`(若 sub01 保留 `mocha_mauve` 作对照)或对 14 个 `Color::Rgb` 字面量。
  - `Config::default().audio().volume()` == 100;`cache().audio_capacity()` == `10*1024^3`;`cache().cover_capacity()` == `1024^3`;`download().quality()` == `BitRate::Lossless`;`sources().netease().timeout_secs()` == 100、`max_connections()` == 0、`proxy()` == None;`daemon().gapless_prefetch_ms()` == 10_000;`tui().behavior()` 五值 == 5/5/30/7/true(对齐 app.rs:33,36,39,42 与 runtime/daemon.rs:25 原 const)。
  - 放 `mineral-config` crate 的测试(依赖 sub01 的 `Config`);本 subspec 关心的是「值映射正确」,故由消费侧 crate 也各加一条镜像断言(如 `mineral-tui` 测 `from_config`)。
- **逐旋钮生效**:
  - `Theme::from_config` 喂一份改了 `accent` / `roles.accent` 的 `ThemeConfig`,`assert_eq!` 输出 token 与 `source_color(PaletteRole::Accent)` 变化(纯函数 `assert_eq!`)。
  - `NeteaseConfig::builder().timeout_secs(7).build()` → 不真打网络,断言 getter == 7(timeout 真实生效无法低成本断言,只验布线;真实 timeout 行为不在单测覆盖)。
  - `AudioHandle::spawn(AudioMode::ForceNull, /*initial_volume*/ 30)` 后 `snapshot().volume_pct == 30`(**`multi_thread` tokio rt** 教训:起 engine 的 test 不能默认 `current_thread`,否则全仓并发 flaky;`#[tokio::test(flavor = "multi_thread", worker_threads = ...)]` 或 std thread + 同步 channel,沿用 `handle.rs:229-258` 既有 test 形态)。
  - **behavior 步长生效**:造 `App`(`test_support::app_with_queue`,`CoverFetcher::disabled()` 零依赖)注入 `volume_step=10` / `seek_step_secs=15` / `list_jump_rows=3` 的 behavior,喂对应 `KeyEvent`(`+` / `Left` / 列表 PageDown),`assert_eq!` 音量增量 == 10、seek 增量 == 15、列表选中行跳 == 3(纯交互断言,跨 tick);默认值(5/5/7)对照一条防回归。`popup/queue.rs` 大步跳行同样从注入值取(验消重后单一真相源)。
  - **退出不杀 daemon(开关为 false)**:`kill_spawned_daemon_on_exit=false` 时 TUI 退出路径(`runtime/daemon.rs`)不调结束 daemon 的逻辑——抽纯函数 `should_kill_spawned(flag: bool, spawned: bool) -> bool` 便于 `assert_eq!`(`should_kill_spawned(false, true) == false`、`(true, true) == true`、`(_, false) == false` 续命/非自拉起均不杀);默认 `true` + 自拉起 → 杀,防回归。
- **config check 快照**(sub00/sub01 主导,此处协调):若 `config check` 展示「有效主题色 / 音量 / 缓存容量」,快照测试走 `mineral_test::assert_snap!` 带**中文 description**(关 prepend_module);快照入 git、`cargo insta review` 人工确认,严禁 `INSTA_UPDATE=always`。
- **降级链不破**:
  - `MINERAL_AUDIO_NULL=1` + config `backend = "auto"` → 仍 `ForceNull`(env 短路);env 不设 + config `backend = "null"` → `ForceNull`(在 `serve.rs` resolve 处加单测,纯函数抽出 `resolve_audio_mode(env_present: bool, cfg_backend: &str) -> AudioMode` 便于 `assert_eq!`)。
  - in-proc daemon e2e(`daemon_lifecycle` 串行组,`.config/nextest.toml` 既有 `daemon-e2e = { max-threads = 1 }`)起一次带非默认 config 的 daemon,`status` 子命令验 backend/volume 不崩(`MINERAL_AUDIO_NULL=1` 确定性)。
- **netease 未登录仍 Ok(None)**:`build_netease` 在无凭证时早返回,不构造 `NeteaseConfig`——加单测覆盖(`load_stored` mock 为 None → `Ok(None)`),证明 config 注入未提前到早返回之前。
- 真实 IO / engine 测试一律 `multi_thread`(memory 既有教训,`docs/testing.md` 重申)。

## 8. 验收判据

1. `cargo build --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` 全绿(含 800 行 / 300 行体量约束、`missing_docs`、无 `unwrap`/`as`)。
2. 不写 `config.lua`:TUI 主题、初始音量(100)、缓存容量、下载音质、netease timeout(100)/proxy(无)/max_connections(0)、gapless 窗口(10s)与改动前**逐值一致**(由 §7「行为不变」测试守)。
3. 写 `config.lua` 改任一旋钮:对应消费点取到新值(由 §7「逐旋钮生效」测试守)。
4. `mineral-config` 两个 `pub const` 已删,全仓无残留引用(`grep` 零命中)。
5. `NeteaseConfig` 为合规配置 struct(私有字段 + `#[non_exhaustive]` + builder + getters),`TIMEOUT_SECS` / `DEFAULT_VOLUME_PCT` / `DOWNLOAD_QUALITY` 三常量退役;behavior 的 5 个 const 退役——`VOLUME_STEP`(app.rs:33)/ `SEEK_STEP_S`(:36)/ `SEEK_BIG_STEP_S`(:39)/ `ROW_BIG_STEP`(:42,**含 `popup/queue.rs:19` 的重复定义一并删除**)/ `KILL_SPAWNED_DAEMON_ON_EXIT`(runtime/daemon.rs:25),全仓无残留引用(`grep` 零命中)。
6. 三条降级链(audio null / cover disabled / netease 未登录)行为不变(由 §7 降级测试 + e2e 守)。
7. `App.theme` 为 `Arc<Theme>`,生产路径无 `Theme::default()`(测试内 `&Theme::default()` 可留)。

## 9. 风险

| 风险 | 缓解 |
|---|---|
| `Config` 子结构 getter 形状由 sub01 定,本文签名假定(`cfg.audio().volume()`)可能与 sub01 实际不符 | open question Q1 提交主对话裁决统一命名;实现期以 sub01 产物为准,签名是示意 |
| `Server::spawn` / `AudioHandle::spawn` / `PlayerCore::spawn` 多处签名连锁改动,跨 PR 中间态编译断 | §6 每 PR「加参数 + 默认 = 旧常量」保持可独立绿;聚合进 `ServerConfig` 减少参数列爆炸 |
| `Theme: Copy` 改 `Arc<Theme>` 触及大量渲染调用点 | 渲染签名收 `&Theme` 不变,只在 `App` 内存 `Arc`,解引用 `&*app.theme` 传入;改动面收敛在 `app.rs` + `lib.rs` |
| env 与 config 优先级 resolve 散落 binary 多处 | 抽 `resolve_audio_mode` 纯函数集中可测;约定「env 只在 binary 边缘读」(serve.rs 既有注释)延续 |
| `mineral-config` const 退役与 sub01 落地的时序耦合 | PR-A commit 拆分:先加 config 路径保留 const、再删 const;sub01 的 `CacheConfig` 是硬前置 |
| netease `proxy = false`(Lua)→ `Option<String>` 映射(母设计 §3:false=禁用) | 该映射在 **sub01** 的 serde 层(`false` → `None`);本文 `NeteaseConfig.proxy: Option<String>` 直接消费,不处理 Lua bool |

## 10. Open Questions(已裁决)

- **Q1**(getter 链命名):**已裁决** ✔ 用**嵌套 getter**(`cfg.audio().volume()`),与本文签名假定一致;sub01 子结构 getter 形如此。
- **Q2**(`audio.backend` 进 `ServerConfig` 还是独立传):**已裁决** ✔ `audio_mode` **独立传**(backend 经 env resolve 后已成 `AudioMode`),`ServerConfig` **不含** backend,避免双真相。
- **Q3**(Phase 0 是否引入新增 CLI flag):**已裁决** ✔ **不引入** 任何新增 CLI flag(如 `--volume`),只保留四级优先级链的架构位置,避免范围蔓延。
- **Q4**(`download.dir` 本期是否接线):**已裁决** ✔ 本期**接线**——在 `open_env` 用 config 值覆盖 `music_export_dir()` 默认;与 env 的优先级为 **env(`MINERAL_DOWNLOAD_DIR`)> config**(env 命中短路 config,对齐既有「env 只在 binary 边缘读」约定)。
- **Q5**(`Theme::mocha_mauve()` / `Default for Theme` 去留):**已裁决** ✔ **保留** `mocha_mauve` **仅供测试对照**(`#[cfg(test)]` 或文档标注),生产构造一律走 `Theme::from_config`。
