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
| `mineral` (bin) | `mineral/` | TUI 入口,ratatui + crossterm 事件循环 |
| `mineral-model` | `crates/mineral-model/` | 跨 channel 的统一数据模型(`Song` / `Album` / `Playlist` / `Lyrics` / `BitRate` / 各 ID newtype 等),所有 channel 的输出在这里平铺合并 |
| `mineral-channel-core` | `mineral-channel/core/` | `MusicChannel` trait + `Page` / `Credential` / `Error` 公共类型,上层只面向此 trait 编程 |
| `mineral-channel-netease` | `mineral-channel/netease/` | 网易云的具体实现,自底向上分层:`crypto → device → transport → dto → api → channel` |
| `mineral-config` | `crates/mineral-config/` | 全局 `MineralConfig`(`once_cell::Lazy`),目前只暴露 `music_dirs`,加载逻辑 TODO |
| `mineral-macros` | `crates/mineral-macros/` | `define_id!` / `define_uuid!` 宏,生成 String-backed、`#[serde(transparent)]` 的 ID newtype |

> 注:`mineral-platform` / `mineral-log` 在 workspace dependencies 里有声明并被 `mineral-config` 引用,但当前 git 里没有 `crates/mineral-platform` / `crates/mineral-log` 目录。如果要构建 `mineral-config`,需要先把这两个 crate 补回(或临时调整 workspace member 集合)。

## 常用命令

```bash
cargo build                                # 构建整个 workspace
cargo build -p mineral                     # 单独构建 TUI binary
cargo run  -p mineral                      # 运行 TUI
cargo test                                 # 跑所有 unit + integration tests
cargo test -p mineral-channel-netease      # 仅跑网易云 crate 的测试
cargo test --test crypto_vectors           # 跑加密自检 harness(byte-for-byte 比对 openssl)
cargo test <name>                          # 按测试名子串过滤
cargo fmt                                  # 格式化(rustfmt)
cargo fmt --check                          # CI 用,不修改
cargo clippy --all-targets -- -D warnings  # 严格 lint
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

### `mineral` (TUI) 现状

当前 `mineral/src/main.rs` 只是个 ratatui 占位骨架(显示一行字、按 q 退出)。设计稿与落地指南在仓库未追踪目录 `design_handoff_tuimu_player/`(MINERAL_IMPLEMENTATION.md + screenshots),做 TUI 工作前先看一遍。

`#[cfg(windows)] compile_error!("Windows暂不支持");` 是有意为之——目前不打算覆盖 Windows。

## Rust 工程约定

> 这些约定是项目级别的硬性要求。Cargo.toml 已经把一部分接到 clippy 里强制执行,其余的没接 lint 但同样要遵守——review/PR 会按这些规则把关。

### 当前 workspace 强制的 clippy lints

`Cargo.toml` 的 `[workspace.lints.clippy]` 段:

```
format_push_string     = "deny"   # 禁止"先 format! 再 push 到 String"
implicit_clone         = "deny"   # 禁止隐式 clone
map_err_ignore         = "deny"   # 禁止 .map_err(|_| ...) 丢上下文
needless_pass_by_value = "deny"   # 传值但未消耗所有权
option_option          = "deny"   # 禁止 Option<Option<T>>
use_self               = "deny"   # 用 Self 而非显式类型名
```

`if_same_then_else` / `len_without_is_empty` / `missing_safety_doc` / `module_inception` 显式 `allow`。

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
* 对外配置类 struct 必须:私有字段 + `#[non_exhaustive]` + builder 构造 + getter 读取。Builder 默认用 `typed-builder`,getter 默认用 `derive-getters`。**禁止**新增可被外部用 `Struct { ... }` 字面量构造的配置 struct。例外:`AgentConfig` 由 `provider::lookup_capabilities` 派生 `context_window`,`typed-builder` 表达不了 fallible + 派生字段——继续用自定义 builder。
* 结构体字段如果带文档或属性,字段块之间留一个空行,避免连续字段的注释/属性挤在一起。

### API 设计

* 不要让调用点出现 `foo(false, None, 30)` 这种谜语写法。优先枚举 / 具名方法 / newtype。
* 新增 trait 必须写文档说明其职责以及实现方应如何使用。

### 体量约束(hook 强制)

* 单文件 ≤ 800 行(不含测试);接近 500 行时就考虑拆,而不是继续涨。
* 单函数 ≤ 300 行;再大就按职责拆子函数。
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
* `mineral-config` 当前依赖 `mineral-platform` / `mineral-log`(workspace dep 已声明,但源码目录可能不在 git 中)。改 config 前先确认这两个 crate 是否齐全。
