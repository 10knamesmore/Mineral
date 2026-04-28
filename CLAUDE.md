# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 仓库总览

Mineral 是一个多源终端音乐播放器(ratatui),目前已落地"模型 + 网易云 channel + TUI 雏形",目标是把本地音乐与多个云源合并为一组统一的 `Vec<Song>` 供 UI 消费。

整个仓库是单一 cargo workspace(根目录 `Cargo.toml`),成员通过 glob 声明:

```
members = ["mineral", "crates/*", "mineral-channel/*"]
```

| crate | 路径 | 职责 |
|---|---|---|
| `mineral` (bin) | `mineral/` | TUI 入口,ratatui + crossterm 同步事件循环 |
| `mineral-model` | `crates/mineral-model/` | 跨 channel 的统一数据模型(`Song` / `Album` / `Playlist` / `Lyrics` / `BitRate` / 各 ID newtype 等),所有 channel 的输出在这里平铺合并 |
| `mineral-channel-core` | `mineral-channel/core/` | `MusicChannel` trait + `Page` / `Credential` / `Error` 公共类型,上层只面向此 trait 编程 |
| `mineral-channel-netease` | `mineral-channel/netease/` | 网易云的具体实现,自底向上分层:`crypto → device → transport → dto → api → channel` |
| `mineral-channel-mock` | `mineral-channel/mock/` | 内嵌假数据的 channel,供 TUI 离线开发/截图使用,通过 `--features mock` 启用 |
| `mineral-macros` | `crates/mineral-macros/` | `define_id!` / `define_uuid!` 宏,生成 String-backed、`#[serde(transparent)]` 的 ID newtype |

## 常用命令

```bash
cargo build                                       # 构建整个 workspace
cargo build -p mineral                            # 单独构建 TUI binary
cargo run  -p mineral                             # 运行 TUI(无数据,等真实 channel 接入)
cargo run  -p mineral --features mock             # 运行 TUI,使用 mineral-channel-mock 喂假数据
cargo test                                        # 跑所有 unit + integration tests
cargo test -p mineral-channel-netease             # 仅跑网易云 crate 的测试
cargo test --test crypto_vectors                  # 跑加密自检 harness(byte-for-byte 比对 openssl)
cargo test <name>                                 # 按测试名子串过滤
cargo fmt                                         # 格式化(rustfmt)
cargo fmt --check                                 # CI 用,不修改
cargo clippy --workspace --all-targets -- -D warnings  # 严格 lint(包括函数体量约束)
```

### 网易云全面联调

`/.cargo/config.toml` 定义了一个 alias:

```bash
cargo apitest                                       # 只跑无登录部分
NETEASE_MUSIC_U=<浏览器 cookie> cargo apitest         # 同时跑登录态用例
```

它实际是 `cargo run -p mineral-channel-netease --example apitest`。无登录部分会真发 HTTPS 请求到网易服务器,所以这条命令需要外网。

### 加密自检

`mineral-channel-netease` 的 `tests/crypto_vectors.rs` 用 `openssl` 作为参考实现,对 WEAPI / EAPI / LINUXAPI 三套算法做 byte-for-byte 比对。改动 `crypto/` 子模块后必须 `cargo test --test crypto_vectors`。

## 架构要点

### 数据模型 = 跨 channel 的契约

`mineral-model` 的设计原则是"平铺合并":除了 `Song::source: SourceKind` 这样的来源标记,模型里没有 channel-specific 字段。任何 channel 实现都把网络/本地原始数据先映射到 `mineral-model` 的类型,再交给上层。这意味着新增 channel 时,**不要**给 `Song` 加 channel-only 字段——要么提升为通用字段,要么保留在 channel 内部 dto 里。

ID 类型(`SongId`、`AlbumId` 等)由 `mineral_macros::define_id!` 生成,内部都是 `String`,带 `#[serde(transparent)]`,跨 channel 互转零成本。需要随机生成的 ID(如本地音乐)用 `define_uuid!`。

### `MusicChannel` trait 是抽象边界

`mineral-channel-core::MusicChannel`(`async_trait`)定义了所有 channel 必须实现的方法:搜索、详情、播放 URL、歌词、可选的登录/用户播放列表。**TUI 只面向这个 trait 编程**,不要直接 import `mineral-channel-netease` 的类型。

错误经过两层:
- channel 内部用 `anyhow::Result`(配 `?` 加 context)。
- 暴露到 trait 边界时,在 `impl MusicChannel` 里通过 `map_err(Error::Other)` 收敛到 `mineral_channel_core::Error`。新增端点遵循同样模式。

### 网易云 channel 的分层(自底向上)

```
crypto/    WEAPI / EAPI / LINUXAPI 三种加密(纯 Rust),公共出口都返回 form body 字符串
device/    deviceId 池、sDeviceId、ChainID 等设备指纹
transport/ isahc HttpClient + cookie jar + UA 池 + URL 改写 + zlib 解压
dto/       网易原生 JSON 结构(serde,内部使用,不外泄)
api/       端点封装(逻辑层,函数式,接受 &Transport)
channel.rs 把 api/ 的方法绑到 MusicChannel trait
```

加成一条经验法则:**任何"看起来像协议层"的改动都要先看 `crypto/` 自检 harness 跑过没**。

### `mineral` (TUI) 架构

设计稿与落地指南在仓库未追踪目录 `design_handoff_tuimu_player/`(MINERAL_IMPLEMENTATION.md + screenshots + 一份 React/HTML 参考实现),改 UI 前先看一遍。

`mineral/src/` 模块职责:

| 文件 / 目录 | 职责 |
|---|---|
| `main.rs` | 入口;`color_eyre::install` → 创建 `Tui` guard → `App::new().run(&mut tui)` |
| `tui.rs` | raw mode + alternate screen 的 RAII guard |
| `app.rs` | 顶层 `App`(`should_quit` / `theme` / `state` / `last_tick`) + 同步事件循环(crossterm `event::poll` + 250ms tick) |
| `state.rs` | `AppState`(playlists / sel_track / current / playback / spectrum / queue / ...) + `View` / `Focus` 枚举 |
| `view_model.rs` | 业务模型 → `PlaylistView` / `SongView` 投影 (UI 渲染只读这一层) |
| `view.rs` | 顶层 `draw(frame, app)` 入口,把 frame 分发给 `components/` |
| `theme.rs` | Catppuccin Mocha 配色 + `with_accent`,UI 不允许写死颜色 |
| `playback.rs` | 播放状态机(playing / paused / pos / shuffle / repeat 等) |
| `cmd.rs` | 命令 / 搜索条(`CmdMode` / `CmdEffect`) |
| `layout.rs` | 响应式布局,**不允许**写死字符数尺寸 |
| `components/` | UI 子组件:`sidebar/`(playlists + tracks 双视图)、`now_playing/`、`overlay/`、`top_status.rs`、`transport.rs`、`cmd_bar.rs`、`spectrum.rs`、`lyrics.rs`、`cover.rs` |

数据来源:`AppState` 在 `--features mock` 时通过 `mineral_channel_mock::MockChannel` 拿假数据;否则两个 cache 是空 `Vec`,UI 照常渲染(只是没歌单)。真实 channel(网易云等)接入到 `AppState` 是 TODO。

`#[cfg(windows)] compile_error!("Windows 暂不支持");` 是有意为之——目前不打算覆盖 Windows。

## Rust 工程约定

> 这些约定是项目级别的硬性要求。Cargo.toml 已经把一部分接到 clippy 里强制执行,其余的没接 lint 但同样要遵守——review/PR 会按这些规则把关。

### 当前 workspace 强制的 lints

完整列表见 `Cargo.toml [workspace.lints.rust]` / `[workspace.lints.clippy]`(随项目演进,以那里为准)。
按职责分组,以下都是 `deny`:

- **panic 类**:`panic` / `unwrap_used` / `expect_used` / `indexing_slicing` / `index_refutable_slice` —— 测试也不豁免,改用 `?` + `assert_*`。
- **数值安全**:`as_conversions` / `cast_lossless` —— 强转用 `TryFrom` / `try_into`。
- **进程安全**:`exit` / `mem_forget`。
- **错误处理**:`map_err_ignore` —— 不许 `.map_err(|_| ...)` 丢上下文。
- **并发**:`mutex_integer` / `maybe_infinite_iter`。
- **所有权 / 性能**:`implicit_clone` / `needless_pass_by_value` / `cloned_instead_of_copied`。
- **代码清晰度**:`branches_sharing_code` / `mismatching_type_param_order` / `option_option` / `wildcard_imports` / `redundant_closure_for_method_calls` / `uninlined_format_args` / `manual_let_else` / `use_self` / `format_push_string`。
- **体量约束**(详见下文):`too_many_lines` —— 阈值 300,在 `clippy.toml`。
- **rust 层**:`warnings = "deny"` / `unsafe_code = "forbid"` / `missing_docs = "deny"`。

显式 `allow`:`if_same_then_else` / `len_without_is_empty` / `missing_safety_doc` / `module_inception`。

### 其余约定(未必经 lint,仍需遵守)

* 禁止 `unsafe_code`、`unwrap`、`expect`、`panic!`、`as`(数值强转)、wildcard import(`use foo::*`)。需要数值转换时用 `TryFrom` / `try_into`;需要快速失败时用 `?` + `anyhow::Context`。
* **不要 `use` 任何 `Result` 类型**。要么定义命名 `type ToolResult = ...`,要么签名直接写 `-> anyhow::Result<T>` / `-> mineral_channel_core::Result<T>`。
* 在测试函数里用 `?` + context 直接返回 `anyhow::Result<()>`,业务断言用 `assert!` / `assert_eq!`,**不要 `unwrap`**。
* 对不透明字面量参数(`None` / `true` / `false` / 数字)写 `/*param_name*/` 注释,名字必须与函数签名完全一致;字符/字符串字面量不需要。
* 类型标注优先 turbofish:写 `Vec::<T>::new()`、`.collect::<Vec<T>>()`,而不是左侧 `: Vec<T>`。例外:trait object 向上转型(`let x: Arc<dyn Trait> = ...`)和无法推断的 `None`(`let x: Option<T> = None`)。
* `format!` / `println!` 等参数尽量内联(clippy `uninlined_format_args`)。
* 优先方法引用而非简单闭包;`match` 尽量穷尽,避免随手写 `_ =>`。

### 文档与可见性

* 所有 `pub` 项必须有 `///` 文档,模块必须有 `//!`,`pub struct` 的每个字段都必须有 `///`。
* `mod.rs` 仅用于模块导出/组织,**不要在 `mod.rs` 里写逻辑**。
* 优先私有模块 + 显式 `pub use` 控制对外 API;不要顺手把内部 type 标 `pub`。
* 对外配置类 struct 必须:私有字段 + `#[non_exhaustive]` + builder 构造 + getter 读取。Builder 默认用 `typed-builder`,getter 默认用 `derive-getters`。**禁止**新增可被外部用 `Struct { ... }` 字面量构造的配置 struct。
* 结构体字段如果带文档或属性,字段块之间留一个空行,避免连续字段的注释/属性挤在一起。

### API 设计

* 不要让调用点出现 `foo(false, None, 30)` 这种谜语写法。优先枚举 / 具名方法 / newtype。
* 新增 trait 必须写文档说明其职责以及实现方应如何使用。

### 体量约束(自动强制)

* **单函数 ≤ 300 行**:由 `clippy::too_many_lines` 强制(阈值在 `clippy.toml`,与其他 lint 一起在 `cargo clippy` 时报错)。
* **单文件 ≤ 800 行(不含 `#[cfg(test)] mod` 块)**:由 `.claude/hooks/check_file_size.py` 在 PostToolUse(Edit / Write / MultiEdit)时强制,> 500 行预警(stderr),> 800 行直接 exit 2 阻止本次工具调用。Hook 注册在 `.claude/settings.json`。
* 从大模块抽代码时,把对应测试和模块/类型文档**一并迁走**,不要留半截。

### 函数注释格式

```rust
/// 简要描述。
///
/// # Params:
///   - `param`: 说明
///
/// # Return:
///   说明
```

## 一些容易踩的点

* **不要为某个 channel 的特殊字段污染 `mineral-model`**——平铺合并是核心契约。
* **改动 `crypto/` 后必跑 `cargo test --test crypto_vectors`**,服务端解不出来不会立刻爆,而是返回 `code != 200` 的 JSON,排查成本高。
* `cargo apitest` 会真打 `music.163.com`;离线环境会失败,这不是 bug。
* TUI 离线开发默认走 `cargo run -p mineral --features mock`;不带 feature 跑也能起来,但 `playlists` / `tracks_cache` 都是空。
* `.claude/hooks/check_file_size.py` 用大括号配平剔除 `#[cfg(test)] mod` 块,字符串字面量里出现 `{` / `}` 可能误判;真遇到再升级到 `syn` AST。
