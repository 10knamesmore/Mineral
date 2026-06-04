# Sub01 · mineral-config crate:loader + schema + LuaCATS(Phase 0 核心)

> 父设计:`2026-06-04-user-config-lua-design.md`(已定宪法,本文只细化落地)。
> 状态:实现级 subspec,open questions 已裁决(见 §12),可进实现计划。
> 本文按项目约定不纳入版本控制。

## 1. 范围与不做

**做(本 subspec 全权负责)**

- `mineral-config` 从"常量箱"长成完整配置 crate:`loader`(mlua eval / 深合并 / from_value)+ `schema`(强类型 `Config` 按域拆模块)+ `lua/default.lua` + `lua/meta/*.lua` stub。
- 新增 workspace 依赖 `mlua`,清理孤儿依赖 `config_rs`(根 `Cargo.toml:41`,全仓零使用 — 已核实)。**本 subspec 是 `mlua` workspace 条目的 canonical 声明**:features 取全仓并集 `["lua54", "vendored", "serialize", "send"]`(`serialize` 供本 crate `from_value`,`send` 是 sub04 VM 跨线程移交脚本线程的前提;workspace 单点声明,各 crate 继承),版本以落地时最新 stable 为准(不钉死在本文档里)。
- `load()` 永不失败的对外 API:返回 `(Config, Vec<ConfigWarning>)`。
- `mineral mineral.on` / `mineral.action` 等 host API 的 **no-op stub 注入点**(非 daemon 进程 eval 时不报错;真正实现是 sub04 的 daemon VM)。
- `mineral config init` / `mineral config check` 子命令(落点在 `mineral-cli`,但子命令逻辑依赖本 crate 的 loader,故在本 subspec 内交付)。
- `.luarc.json` 生成。
- 退役两个 `pub const`(`lib.rs:7,10`),改由 `Config` 提供。

**不做(交界划清)**

- **各 crate 接线** = sub02:`Theme::from_config`、TUI keymap 解析、`Action` 枚举统一、server 把 `Config` 注入 MediaCache/download/player/engine、netease `NeteaseConfig` 加 `timeout_secs` 字段并接线。本 subspec 只**定义 schema 字段 + 反序列化目标类型**,不改 `mineral-tui`/`mineral-server`/`mineral-channel-netease` 的运行期代码。
- **daemon VM 实运行时**(host API 真实绑定 / hooks / observe / 看门狗)= sub04 的 `mineral-script`。本 subspec 只放 no-op stub 与"VM 由谁注入真实表"的注入点契约。
- **协议层**(`Event`/`KeyContext`/握手)= sub03。
- 热重载、web/nvim frontend = Phase 2+,不碰。

**边界约定(供一致性检查)**:本 subspec 产出的 `Config` 及子 struct(私有字段 + getters)是 sub02/sub04 的**唯一消费契约**;它们只读 getter,不得绕过构造字面量。`default.lua` 字段集 ⇔ `Config` 字段集由守卫测试钉死。

## 2. 现状锚点(file:line,均已核对)

- `crates/mineral-config/src/lib.rs:7,10` — 退役目标:`AUDIO_CACHE_CAPACITY` / `COVER_CACHE_CAPACITY` 两个 `pub const`。
- `crates/mineral-config/Cargo.toml:11` — 当前 `[dependencies]` 为空。
- 根 `Cargo.toml:41` — `config_rs = { package = "config", version = "0.15" }` 孤儿依赖(无任何 crate 在 `[dependencies]` 引用,已 grep 确认;`pub use config::...` 是各 crate 的内部 `mod config`,非此 crate)。
- 根 `Cargo.toml:98-100` — 已有 `serde`(derive)/ `serde_json` / `serde_path_to_error`,可直接复用。
- `crates/mineral-paths/src/lib.rs:18` — `config_dir()`(`xdg.rs:40-42`),loader 据此拼 `config.lua`。
- `crates/mineral-paths/src/lib.rs:64-71` — `music_export_dir()`,download.dir 缺省回落目标。
- `crates/mineral-tui/src/render/theme.rs:59-76` — 14 token 硬编码(`mocha_mauve()`);`:50-56` 三个 `PaletteRole` → 颜色映射(roles)。schema theme 段对齐此处字段名。
- `crates/mineral-audio/src/engine.rs:50` — `DEFAULT_VOLUME_PCT: u8 = 100`;`handle.rs:25-31` — `AudioMode { Auto, ForceNull }`。
- `crates/mineral-server/src/download.rs:35` — `DOWNLOAD_QUALITY: BitRate = BitRate::Lossless`;`crates/mineral-model/src/bitrate.rs:13-26` — `BitRate` serde `rename_all = "lowercase"`,可直接做 schema 字段类型。
- `crates/mineral-channel/netease/src/config.rs:3-8` — `NeteaseConfig { max_connections, proxy }`(**无** `timeout_secs`);`transport/client.rs:22` — `TIMEOUT_SECS: u64 = 100` 是常量(提升为字段是 sub02 接线工作,本 subspec 只在 schema 留 `timeout_secs` 字段)。
- `crates/mineral-server/src/player.rs:35` — `PREFETCH_LEAD_MS: u64 = 10_000`(daemon.gapless_prefetch_ms)。
- `crates/mineral-cli/src/core.rs:38-54` — `Command` enum 落点(加 `Config { cmd: ConfigCommand }`)。
- `crates/mineral-cli/src/subcommands/cache/render.rs` — 纯函数渲染 + comfy-table 的 `config check` 输出范式参考。
- `crates/mineral-test/src/macros.rs:16` — `assert_snap!`(强制中文 description,关 `prepend_module_to_snapshot`)。
- `.config/nextest.toml:9-13` — `daemon-e2e` 串行组(本 subspec **不**触发,纯库测 + CLI 测)。
- `crates/mineral-log/src/lib.rs:45` — `chain(e: impl Display) -> String`,错误日志用。

## 3. 新增 / 修改文件清单

守"单文件 ≤ 800 行(不含 `#[cfg(test)]`)、`mod.rs`/`lib.rs` 不写逻辑、pub 必带 `///`、对外配置 struct 私有字段 + `#[non_exhaustive]` + getters"。

| 文件 | 职责 | 预估行 |
|---|---|---|
| `Cargo.toml`(根) | 删 `config_rs`;加 canonical `mlua = { version, features = ["lua54","vendored","serialize","send"] }` 到 workspace deps(并集,含 sub04 所需 `send`;版本落地时取最新 stable) | ±3 |
| `crates/mineral-config/Cargo.toml` | 加依赖:mlua / serde / serde_path_to_error / mineral-paths / mineral-model / mineral-log / color-eyre;dev:mineral-test / insta / proptest | ~20 |
| `crates/mineral-config/src/lib.rs` | **仅模块导出**:`pub use` 出 `Config`/`TuiConfig`/`load`/`ConfigWarning`/各域子 struct(`ThemeConfig`/`KeysConfig`/`BehaviorConfig`/`AudioConfig`/…);退役旧 const(改 `Config::default()` 提供) | ~25 |
| `crates/mineral-config/src/schema/mod.rs` | schema 模块组织 + `pub use`(不写逻辑) | ~20 |
| `crates/mineral-config/src/schema/config.rs` | 顶层 `Config`(聚合各域)+ `TuiConfig`(client 段聚合 theme + keys + behavior,主 spec D12)+ `Deserialize`/`deny_unknown_fields` + getters | ~140 |
| `crates/mineral-config/src/schema/theme.rs` | `ThemeConfig`(14 token + roles)/ `RolesConfig` / `HexColor`(挂在 `TuiConfig` 下) | ~180 |
| `crates/mineral-config/src/schema/keys.rs` | `KeysConfig`(聚合在 `TuiConfig` 下,经 `cfg.tui().keys()` 取)+ `KeyBinding`(单键 or 数组,自定义 enum 解析)。**键字符串 → 和弦的解析复用 sub00 落在 `mineral-config::keys` 的 `KeyChord` 与 `KeyChord::parse`,不在 schema 重复定义**(sub00 PR-A 先落 `keys` 模块的纯类型;本 schema `use crate::keys::KeyChord`)。 | ~120 |
| `crates/mineral-config/src/schema/behavior.rs` | `BehaviorConfig`(聚合在 `TuiConfig` 下,经 `cfg.tui().behavior()` 取):5 个交互手感旋钮(volume_step / seek_step_secs / seek_big_step_secs / list_jump_rows / kill_spawned_daemon_on_exit)。单拆模块与 `keys.rs` 解耦(交互手感 vs 键位映射),守 `keys.rs` 行数预算。 | ~50 |
| `crates/mineral-config/src/schema/audio.rs` | `AudioConfig`(volume / backend);`BackendKind` enum | ~90 |
| `crates/mineral-config/src/schema/cache.rs` | `CacheConfig`(audio_capacity / cover_capacity) | ~70 |
| `crates/mineral-config/src/schema/download.rs` | `DownloadConfig`(quality:复用 `BitRate` / dir:`Option<PathBuf>`) | ~80 |
| `crates/mineral-config/src/schema/sources.rs` | `SourcesConfig` / `NeteaseSection`(timeout_secs / proxy / max_connections) | ~110 |
| `crates/mineral-config/src/schema/daemon.rs` | `DaemonConfig`(gapless_prefetch_ms;预留看门狗阈值字段) | ~80 |
| `crates/mineral-config/src/loader/mod.rs` | loader 模块组织 + `pub use load`(不写逻辑) | ~15 |
| `crates/mineral-config/src/loader/pipeline.rs` | `load()` 主管线:eval default → eval user → merge → from_value;收 warnings | ~180 |
| `crates/mineral-config/src/loader/merge.rs` | 调用内置 Lua merge 函数 + 注入 merge 实现的胶水 | ~60 |
| `crates/mineral-config/src/loader/stub.rs` | no-op host API stub 注入(`mineral.on`/`action`/`bind`/`observe`/`ui.toast`/`log.*`),供非 daemon eval | ~120 |
| `crates/mineral-config/src/loader/warning.rs` | `ConfigWarning` 类型 + Display(file:line / 字段路径) | ~90 |
| `crates/mineral-config/src/lua/default.lua` | 内置默认配置全文(LuaCATS 注解),`include_str!` | ~140 |
| `crates/mineral-config/src/lua/merge.lua` | 深合并函数(数组整体替换),`include_str!` | ~50 |
| `crates/mineral-config/src/lua/meta/mineral.lua` | host API `---@meta` stub(分发到用户目录) | ~120 |
| `crates/mineral-config/src/lua/meta/config.lua` | `---@class mineral.Config` 及子 class 注解 stub | ~150 |
| `crates/mineral-config/src/init.rs` | `config init` 落地:写 config.lua 模板 + .luarc.json + 拷 meta stub | ~140 |
| `crates/mineral-config/src/check.rs` | `config check` 纯函数:load → 渲染诊断(供快照) | ~120 |
| `crates/mineral-cli/src/subcommands/config/mod.rs` | `ConfigCommand` enum + 分发(不写逻辑,转调 mineral-config) | ~30 |
| `crates/mineral-cli/src/subcommands/config/command.rs` | clap 子命令定义 + 调用 `init`/`check` | ~80 |
| `crates/mineral-cli/src/core.rs`(改) | `Command` 加 `Config` 变体 + 分发 | ±10 |

> `theme.rs`(theme 14 token + HexColor)接近 180 行预估;若 keys 内建动作枚举膨胀超 500 行预警线,把 `KeyBinding` 解析单拆 `keys/binding.rs`。`TuiConfig` 本体只是 theme + keys + behavior 的聚合壳,落 `config.rs` 与顶层 `Config` 同文件(几十行,不单拆模块,守 800 行无虞)。

## 4. 关键类型与签名

### 4.1 顶层 Config 与 load 入口

```rust
/// 用户运行期配置的强类型真相源。`default.lua` 与用户 `config.lua` 深合并后,
/// 整表一次 `from_value` 落成本类型;各域子段见各自 getter。
///
/// 字段私有 + `#[non_exhaustive]`:外部只能经 [`load`] 或 [`Config::defaults`] 取得,
/// 经 getter 读取,不可字面量构造(对外配置 struct 约定)。
#[derive(Clone, Debug, serde::Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Config {
    /// TUI client 段(主 spec D12):in-repo client 专属命名空间,内含 theme + keys。
    /// 协议上无特权,仅打包特权;未来 in-repo client 平行加段(`web = {}`)。
    tui: TuiConfig,
    /// 音频段(音量 / 后端)。
    audio: AudioConfig,
    /// 缓存容量段。
    cache: CacheConfig,
    /// 下载段。
    download: DownloadConfig,
    /// 音乐源段。
    sources: SourcesConfig,
    /// daemon 段(gapless / 看门狗)。
    daemon: DaemonConfig,
}

/// TUI client 配置命名空间(主 spec D12)。把原本顶层平级的 `ui`/`keys` 收进
/// client 段:TUI 是 in-repo client,在协议上无特权,只有"打包特权"。第三方
/// client(如 nvim 插件)的配置活在自己生态,不进本文件;未来 in-repo client
/// 平行加段(`web = {}`)。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取:`cfg.tui().theme()` / `cfg.tui().keys()`。
#[derive(Clone, Debug, serde::Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct TuiConfig {
    /// 主题色板段(14 token + 3 roles)。
    theme: ThemeConfig,
    /// 键位重映射段(动作 → 键)。
    keys: KeysConfig,
    /// 交互手感段(音量/seek 步长、列表跳行、daemon 续命开关)。
    behavior: BehaviorConfig,
}

/// 加载用户配置,**永不失败**:任何错误降级为纯默认 + 一条 [`ConfigWarning`]。
///
/// 管线:`include_str!` 内置 `default.lua` eval → 用户 `config.lua` eval(缺失则跳过)
/// → Lua 层深合并 → `from_value::<Config>`。非 daemon 进程注入 no-op host stub
/// (见 [`crate::loader`]),故顶层 `mineral.on(...)` 调用安全。
///
/// # Params:
///   - `user_path`: 用户配置文件路径(通常 `config_dir()/config.lua`);不存在视为纯默认。
///
/// # Return:
///   `(Config, Vec<ConfigWarning>)`:warnings 非空 = 用了默认兜底,调用方据此 toast。
pub fn load(user_path: &std::path::Path) -> (Config, Vec<ConfigWarning>) { /* ... */ }

impl Config {
    /// 纯默认配置(eval `default.lua`)。仅在守卫测试与降级路径用;
    /// 业务正常路径走 [`load`]。
    ///
    /// # Return:
    ///   内置默认;若 default.lua 自身坏(不该发生,有守卫测试)返回 `Err`。
    pub fn defaults() -> color_eyre::Result<Self> { /* ... */ }
}
```

> **已裁决(Q1)**:Rust 侧**不实现** `Default`(避免与 default.lua 双源漂移),缺字段由 default.lua 在 Lua 层深合并补齐后整表必全;`deny_unknown_fields` 保留,顶层**不加** `#[serde(default)]`。`Config::defaults()` 走 eval default.lua。上方 struct 示意里的 `#[serde(deny_unknown_fields, default)]` 应去掉 `default`。

### 4.2 ConfigWarning(永不失败的代价)

```rust
/// 加载过程中的非致命问题。出现即表示该层(或整份)配置回落了默认。
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ConfigWarning {
    /// 用户 `config.lua` eval 失败(语法错 / 运行期错)。`detail` 含 file:line。
    Eval {
        /// 人类可读详情(Lua 错误首行 + 定位)。
        detail: String,
    },
    /// 合并后 `from_value::<Config>` 失败。`path` 是 serde_path_to_error 字段路径。
    Deserialize {
        /// 出错字段路径,如 `audio.volume`。
        path: String,
        /// 类型 / 取值错误详情。
        detail: String,
    },
}
```

`ConfigWarning` 实现 `Display`(单行,供 toast)；日志侧调用方用 `mineral_log::chain` 不适用(这不是 `Report`),改用 `Display`。

### 4.3 域子 struct 示例(theme / audio)

```rust
/// 主题色板:14 个 color token + 3 个语义角色映射。对齐 `theme.rs:59-76`。
#[derive(Clone, Debug, serde::Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ThemeConfig {
    /// 主背景 `#rrggbb`。
    base: HexColor,
    /* … 其余 13 token … */
    /// 语义角色 → token 名映射(accent / muted / faint)。
    roles: RolesConfig,
}

/// `#rrggbb` 十六进制颜色 newtype。反序列化即校验格式;不暴露内部表示。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HexColor {
    /// 解析后的 RGB 三元组。
    rgb: [u8; 3],
}

impl<'de> serde::Deserialize<'de> for HexColor {
    /* 解析 "#rrggbb" → [u8;3];非法格式返 de::Error,经 serde_path_to_error 带路径 */
}

impl HexColor {
    /// 取 RGB 三元组(sub02 的 `Theme::from_config` 据此造 `ratatui::style::Color::Rgb`)。
    pub fn rgb(&self) -> [u8; 3] { self.rgb }
}

/// 音频段。
#[derive(Clone, Debug, serde::Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AudioConfig {
    /// 初始音量百分比 0-100。对齐 `engine.rs:50`。
    volume: u8,
    /// 后端选择。
    backend: BackendKind,
}

/// 音频后端选择。对齐 `AudioMode`(`handle.rs:25`),但保持 config 与 audio crate 解耦
/// —— sub02 在接线处做 `BackendKind → AudioMode` 映射,本枚举不依赖 mineral-audio。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum BackendKind {
    /// 自动探测,无设备降级 Null(默认)。
    Auto,
    /// 强制空跑(无声卡)。
    Null,
}

/// TUI 交互手感段(挂在 `TuiConfig` 下,经 `cfg.tui().behavior()` 取)。const 审计
/// 补录的 5 个旋钮:音量/seek 步长、列表大步跳行、自拉起 daemon 的退出续命开关。
/// 接线点 = sub00 的 `VolumeDelta`/`SeekDelta`/`SelectionMove` Action 参数(前四个)+
/// TUI 退出路径(`kill_spawned_daemon_on_exit`),详见 sub02。
#[derive(Clone, Debug, serde::Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct BehaviorConfig {
    /// 单次音量增减步长(百分点)。对齐 `app.rs:33` VOLUME_STEP。
    volume_step: u8,

    /// 单次 seek 步长(秒)。对齐 `app.rs:36`。
    seek_step_secs: u32,

    /// 大步 seek 步长(秒)。对齐 `app.rs:39`。
    seek_big_step_secs: u32,

    /// 列表大步跳行的行数(PageUp/Down 等)。对齐 `app.rs:42` ROW_BIG_STEP
    /// (现 `popup/queue.rs:19` 重复定义,sub02 接线时消重)。
    list_jump_rows: u16,

    /// TUI 退出时是否杀掉自己拉起的 daemon。对齐 `runtime/daemon.rs:25`
    /// KILL_SPAWNED_DAEMON_ON_EXIT;`false` = 自拉起的 daemon 续命。
    kill_spawned_daemon_on_exit: bool,
}
```

> `download.quality` 直接复用 `mineral_model::BitRate`(`bitrate.rs` serde lowercase,已契合 schema);`download.dir` 为 `Option<std::path::PathBuf>`(`nil` → `None` → sub02 回落 `music_export_dir()`)。`sources.netease.proxy` 为枚举/联合表达"false=禁用 / 字符串=URL"(见 Q2)。

### 4.4 loader 管线签名

```rust
/// 在给定 Lua VM 上 eval 内置 default.lua,返回默认表。VM 已注入 no-op stub。
fn eval_default(lua: &mlua::Lua) -> color_eyre::Result<mlua::Table> { /* include_str! */ }

/// eval 用户文件(若存在),返回用户表;eval 失败转 `ConfigWarning::Eval`。
fn eval_user(lua: &mlua::Lua, path: &std::path::Path)
    -> Result<Option<mlua::Table>, ConfigWarning> { /* ... */ }

/// 调用内置 merge.lua:`merged = deep_merge(default, user)`(数组整体替换)。
fn deep_merge(lua: &mlua::Lua, default: mlua::Table, user: mlua::Table)
    -> color_eyre::Result<mlua::Table> { /* ... */ }

/// 合并表 → 强类型,错误带字段路径(serde_path_to_error 风格)。
fn from_lua_value(lua: &mlua::Lua, merged: mlua::Table)
    -> Result<Config, ConfigWarning> { /* lua.from_value + path 包装 */ }
```

> mlua 的 `LuaSerdeExt::from_value` 不直接接 `serde_path_to_error`。**策略**:`from_value` 经 `mlua::Value` → 若需精确字段路径,先 `serde_json` 把 Lua 表转中间 `serde_json::Value`(经 mlua serialize feature)再 `serde_path_to_error::deserialize`。具体取舍见 Q3,优先保证错误含字段名。

### 4.5 no-op stub 注入点(sub04 复用契约)

```rust
/// 在 VM 注入非 daemon 进程的 host API stub:`mineral` 全局表 + 子表
/// `player`/`ui`/`log`,其中 `on`/`action`/`bind`/`observe`/命令族均为 no-op
/// (吞参数、返 nil/假 handle),保证用户 `config.lua` 顶层调用不报错。
///
/// daemon 进程**不**调本函数,改由 `mineral-script`(sub04)注入活实现 —— 二者
/// 同名同形,顶层副作用在 daemon 与非 daemon 进程语义一致(声明面)/分歧(hook 面)。
///
/// # Params:
///   - `lua`: 目标 VM。
pub fn inject_noop_host(lua: &mlua::Lua) -> color_eyre::Result<()> { /* ... */ }
```

`inject_noop_host` 是 **本 subspec 的对外注入点**:sub04 的 daemon 在自己的 VM 上注入活实现并**跳过**此函数;非 daemon(TUI/CLI/守卫测试)经 `load()` 内部调它。

### 4.6 init / check

```rust
/// 生成用户配置模板 + `.luarc.json` + 拷贝 meta stub 到 `<config_dir>/lua/meta/`。
/// 已存在的 config.lua 不覆盖(返回提示而非报错)。
///
/// # Return:
///   写入 / 跳过的文件清单(供 CLI 打印)。
pub fn run_init(config_dir: &std::path::Path) -> color_eyre::Result<Vec<InitOutcome>> { /* ... */ }

/// 加载 + 校验配置,渲染诊断文本(纯函数,供快照)。`color` 控 ANSI。
///
/// # Return:
///   多行诊断:有效配置摘要 + warnings(各带 file:line / 字段路径)。
pub fn render_check(config: &Config, warnings: &[ConfigWarning], color: bool) -> String { /* ... */ }
```

## 5. default.lua 草案(对齐主 spec §5)

```lua
---@meta-off
---@type mineral.Config
-- Mineral 默认配置。用户 config.lua 经深合并覆盖此表(数组整体替换)。
-- 顶层只做纯计算,勿在此放副作用(多进程各 eval 一次,见设计 D8)。
return {
  -- tui 段(主 spec D12):in-repo client 专属命名空间,协议无特权、仅打包特权。
  -- 未来 in-repo client 平行加段(web = {});第三方 client 配置不进本文件。
  tui = {
    theme = {
      base = "#1e1e2e", mantle = "#181825", crust = "#11111b",
      surface0 = "#313244", surface1 = "#45475a", overlay = "#6c7086",
      subtext = "#a6adc8", text = "#cdd6f4",
      accent = "#cba6f7", accent_2 = "#74c7ec",
      red = "#f38ba8", yellow = "#f9e2af", green = "#a6e3a1", peach = "#fab387",
      roles = { accent = "red", muted = "subtext", faint = "overlay" },
    },
    keys = {
      play_pause = "space", next = "n", prev = "p",
      -- 全量内建动作(枚举)在 sub02 Action 统一后定稿;Phase 0 至少接通上述三键 + 音量/seek。
    },
    behavior = {
      volume_step = 5,                     -- app.rs:33
      seek_step_secs = 5,                  -- app.rs:36
      seek_big_step_secs = 30,             -- app.rs:39
      list_jump_rows = 7,                  -- app.rs:42(与 popup/queue.rs:19 重复,sub02 消重)
      kill_spawned_daemon_on_exit = true,  -- runtime/daemon.rs:25
    },
  },
  audio  = { volume = 100, backend = "auto" },
  cache  = { audio_capacity = 10 * 1024 ^ 3, cover_capacity = 1 * 1024 ^ 3 },
  download = { quality = "lossless", dir = nil },
  sources = { netease = { timeout_secs = 100, proxy = false, max_connections = 0 } },
  daemon  = { gapless_prefetch_ms = 10000 },
}
```

LuaCATS 注解(`---@class` / `---@field`)落 `lua/meta/config.lua`(stub),`default.lua` 顶部仅 `---@type mineral.Config` 引用,避免注解文本两份漂移。`10 * 1024 ^ 3` 是 Lua 浮点;`from_value` 时 `u64` 字段需在解析层 floor + 校验非负(见 Q4)。

> **roles 值是 token 名字符串**(`"red"` 指向同表的 token),sub02 的 `Theme::from_config` 解析 `roles.accent = "red"` → 取 `theme.red` 的颜色;本 subspec 的 schema 仅校验 roles 值 ∈ 14 个 token 名集合(`RolesConfig` 反序列化时 validate)。

## 6. merge.lua 草案(深合并 / 数组整体替换)

```lua
-- deep_merge(base, override):返回新表;override 的键覆盖 base。
-- 两侧同键且都是「map 表」→ 递归深合并;否则(标量 / 数组 / 类型不一)→ override 整体替换。
-- 数组判定:序列表(连续整数键从 1 起)视为数组,整体替换不逐元素合并(设计 D3)。
local function is_array(t) --[[ # 连续整数键检测 ]] end
local function deep_merge(base, override) --[[ # 递归 ]] end
return deep_merge
```

`is_array` 的边界(空表 `{}` 既像数组又像 map):空 override 表对 map base → 视为 map 合并(空合并 = 不变),保证 proptest 不变量 `merge(d, {}) == d`。这条边界写进注释 + 测试。

## 7. ---@meta stub 组织与分发

- 仓库内:`crates/mineral-config/src/lua/meta/{mineral,config}.lua`,随 crate 走、`include_str!` 进二进制(供 init 时落地)。
- `mineral.lua`:host API 签名(`---@class mineral` + `mineral.on` / `mineral.action` / `mineral.observe` / `mineral.player.*` / `mineral.ui.toast` / `mineral.log.*`)。Phase 0 这些是 no-op,但 stub 先把签名/文档写全(用户编辑器有补全),实现随 sub04。
- `config.lua`:`---@class mineral.Config`(顶层含 `tui` / `audio` / `cache` / `download` / `sources` / `daemon` 字段)及子 class——`mineral.TuiConfig`(主 spec D12:嵌 `theme` + `keys` + `behavior`)、`mineral.ThemeConfig`、`mineral.KeysConfig`、`mineral.BehaviorConfig`(`volume_step` / `seek_step_secs` / `seek_big_step_secs` / `list_jump_rows` / `kill_spawned_daemon_on_exit`)、`mineral.AudioConfig`、`mineral.CacheConfig`、`mineral.DownloadConfig`、`mineral.SourcesConfig`(含 `mineral.NeteaseSection`)、`mineral.DaemonConfig`,每字段 `---@field`(含取值范围注释)。**这是 default.lua 的注解真相源**。
- 分发:`config init` 把两份 stub 拷到 `<config_dir>/lua/meta/`,并写 `.luarc.json`:
  ```json
  { "runtime.version": "Lua 5.4",
    "workspace.library": ["lua/meta"],
    "diagnostics.globals": ["mineral"] }
  ```
  `workspace.library` 用**相对路径** `lua/meta`(相对 config_dir),便于 dotfile 管理器迁移。

## 8. 实现步骤(依赖顺序,可拆 PR)

1. **PR-A 依赖卫生**:根 `Cargo.toml` 删 `config_rs`、加 `mlua`;`mineral-config/Cargo.toml` 补依赖。验证 `cargo build -p mineral-config` 通过(空骨架)。
2. **PR-B schema**:`schema/*` 全部子 struct + getters + `deny_unknown_fields`;`HexColor`/`BackendKind`/`RolesConfig` 自定义反序列化。退役旧 const(`lib.rs` 改导出 `Config`)。单测:各 newtype 解析 + 非法值报错。
3. **PR-C lua 资产 + loader**:`lua/default.lua`、`lua/merge.lua`、`loader/*`、`inject_noop_host`。守卫测试(default eval → from_value 必成功)、merge 单测 + proptest。
4. **PR-D 错误路径**:`ConfigWarning` + `load()` 降级;坏 Lua / 类型错 → 默认兜底 + warning 含字段路径。
5. **PR-E init/check + CLI**:`init.rs`/`check.rs`/meta stub/`.luarc.json`;`mineral-cli` 加 `Config` 子命令;`config check` 快照测试。

> PR-B/-C 是核心,-A 是前置,-D/-E 可并行于 -C 之后。每个 PR 自带测试,绿了再合。

## 9. 测试清单(对齐 docs/testing.md)

- **default.lua 守卫(核心,nextest)**:`Config::defaults()` = eval default.lua + `from_value::<Config>` 必须 `Ok`;断言每个域 getter 值等于现状常量(volume==100 / quality==Lossless / audio_capacity==10GiB / gapless==10000;behavior:volume_step==5 / seek_step_secs==5 / seek_big_step_secs==30 / list_jump_rows==7 / kill_spawned_daemon_on_exit==true),钉死与 `theme.rs:59-76`、`engine.rs:50`、`download.rs:35`、`player.rs:35`、`app.rs:33,36,39,42`、`runtime/daemon.rs:25` 不漂移。
- **deny_unknown_fields**:default.lua 多写字段 → 守卫测试当场红(隐式覆盖);另写显式测:用户表含未知键 → `ConfigWarning::Deserialize` 且 path 含该键。
- **merge 单测**:partial 覆盖单字段、嵌套深合并(`tui.theme.accent` 单覆不动其余 token;`tui.behavior.volume_step = 10` 单覆不动 `seek_step_secs` 等其余 behavior 旋钮)、数组整体替换(`tui.keys.next = {"n","j"}` 替换非追加)、用户文件缺失 → 纯默认。
- **merge proptest 不变量**(`crates/mineral-config/tests/merge_props.rs`):`merge(d, {}) == d`;`merge(d, d) == d`(幂等);`merge(_, u)` 在 u 覆盖的键上等于 u。生成器造嵌套 Lua 表;失败种子进 git(`proptest-regressions/`)。
- **错误路径**:坏 Lua 语法 → `ConfigWarning::Eval` 且 `Display` 含行号;类型错(`volume = "loud"`)→ `ConfigWarning::Deserialize` 且 path == `audio.volume`;断言 `load()` 仍返回默认 `Config`(永不失败)。
- **HexColor / RolesConfig 单测**:`"#1e1e2e"` 解析;`"1e1e2e"`(缺#)/`"#xyz"` 报错;roles 值非 token 名 → 报错带路径。
- **config check 快照**(`assert_snap!` 带中文 description,关 module prefix):有效配置 / 含 warning 两种输入,`color=false` 渲染。沿用 `cache/render.rs` 纯函数范式。
- **config init**:写临时 config_dir,断言生成 config.lua / .luarc.json / meta/*.lua,且已存在 config.lua 不覆盖(返 skip 而非 err)。用 tempfile,沿 `mineral-paths` 测试的 EnvGuard 范式隔离 `XDG_CONFIG_HOME`。
- **本 subspec 不起 daemon、不真 IO/engine**:纯库测 + CLI(`assert_cmd` 可选),**不进 daemon-e2e 串行组**;无 `multi_thread` rt 需求(无网络 / 无 audio engine)。`inject_noop_host` 的活实现 e2e 归 sub04。

## 10. 验收判据

1. `cargo build -p mineral-config` 通过;`config_rs` 不再出现在 `cargo tree`,`mlua`(vendored)进编译链且 CI 缓存命中。
2. 不写 `config.lua` 时:`load(absent_path)` 返回 `(Config::defaults(), [])`,各旋钮值 == 现状常量(守卫测试绿)。
3. 写 `config.lua` 覆盖某旋钮(如 `audio.volume = 50`):`load()` 返回的 `Config.audio().volume() == 50`,其余字段仍为默认(深合并验证)。
4. 坏配置(语法/类型错):`load()` 不 panic、返回默认 + 非空 warnings,warning `Display` 含 file:line 或字段路径。
5. `mineral config init` 在干净 config_dir 生成模板 + .luarc.json + meta stub;`mineral config check` 输出与快照一致。
6. 用户 `config.lua` 顶层写 `mineral.on("track_finished", function() end)` 在 TUI/CLI 进程 eval **不报错**(no-op stub 生效)。
7. 旧 `pub const AUDIO_CACHE_CAPACITY` / `COVER_CACHE_CAPACITY` 退役;调用方迁移由 sub02 完成(本 subspec 保留 `Config` getter 作迁移目标,**不破坏现有调用** — 见 Q5)。

## 11. 风险

| 风险 | 缓解 |
|---|---|
| mlua vendored 拖慢 CI 首次编译 | vendored 一次性 + CI 缓存(`libasound2-dev` 先例已证可行);本地开发增量编译不受影响 |
| `from_value` 错误丢字段路径 | 经 serde_json 中间值 + serde_path_to_error 包装(Q3);守卫测试断言 path 文本 |
| Lua 数值是 f64,`u64` 容量/字节字段精度 / 负值 | 反序列化层 floor + 范围校验,非法转 warning;`10*1024^3` 在 f64 精确可表 |
| default.lua 与 config stub 注解漂移 | 注解单源(config.lua stub)+ default.lua 仅 `---@type` 引用;守卫测试 + review |
| `is_array` 空表歧义破坏 merge 不变量 | 空表按"不改变 base"语义处理 + proptest `merge(d,{})==d` 钉死 |
| no-op stub 与 sub04 活实现签名漂移 | `inject_noop_host` 的表形状 = sub04 的注入契约,stub 签名以 meta/mineral.lua 为准,两边引用同一份 |

## 12. Open Questions(已裁决)

- **Q1**(子 struct 是否实现 Rust `Default`):**已裁决** ✔ **不实现** Rust `Default`,默认必经 `default.lua` eval(单一真相源)。顶层不加 `#[serde(default)]`,`deny_unknown_fields` 保留。`Config::defaults()` 走 eval `default.lua` 而非 `Default::default()`;内置 `default.lua` 损坏属程序员错误,由守卫测试(§9 default.lua 守卫)拦截,启动期可 `fail`(而非降级)。
- **Q2**(`sources.netease.proxy` 的 `false | "url"` 映射):**已裁决** ✔ 用 **自定义 `Deserialize`**(不用 `#[serde(untagged)]`,避免其错误路径差),`false → None` / `"url" → Some(url)`,非法值经 `serde_path_to_error` 带路径。
- **Q3**(`from_value` 错误路径方案):**已裁决** ✔ 经 `serde_json::Value` 中转 + `serde_path_to_error::deserialize`(多一次转换换取精确字段路径)。配置加载是冷路径,额外开销可接受。
- **Q4**(f64→u64 等非法值的回落粒度):**已裁决** ✔ 反序列化失败**整份回落默认**(对齐主 spec D9),**不做逐字段回落**(部分回落会增加 merge 复杂度,不值得)。非法值产 `ConfigWarning::Deserialize`(带字段路径)+ `load()` 返回纯默认 `Config`。
- **Q5**(旧 `pub const` 退役迁移窗口):**已裁决** ✔ 本 subspec **保留** `pub const` 临时转发到 `Config::defaults()` 对应值(避免 sub02 接线前 server/cli 编译断),由 **sub02 的接线 PR** 删除。
