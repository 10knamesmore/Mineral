# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 仓库总览

Mineral 是一个多源终端音乐播放器(ratatui),目前已落地"模型 + 网易云 channel + TUI 雏形",目标是把本地音乐与多个云源合并为一组统一的 `Vec<Song>` 供 UI 消费。

整个仓库是单一 cargo workspace(根目录 `Cargo.toml`),成员通过 glob 声明:

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

## 架构要点

### 数据模型 = 跨 channel 的契约

`mineral-model` 的设计原则是"平铺合并":除了 `Song::source: SourceKind` 这样的来源标记,模型里没有 channel-specific 字段。任何 channel 实现都把网络/本地原始数据先映射到 `mineral-model` 的类型,再交给上层。这意味着新增 channel 时,**不要**给 `Song` 加 channel-only 字段——要么提升为通用字段,要么保留在 channel 内部 dto 里。

ID 类型(`SongId`、`AlbumId` 等)由 `mineral_macros::define_id!` 生成,内部都是 `String`,带 `#[serde(transparent)]`,跨 channel 互转零成本。需要随机生成的 ID(如本地音乐)用 `define_uuid!`。

### `MusicChannel` trait 是抽象边界

`mineral-channel-core::MusicChannel`(`async_trait`)定义了所有 channel 必须实现的方法:搜索、详情、播放 URL、歌词、可选的登录/用户播放列表。**TUI 只面向这个 trait 编程**.。

## Rust 工程约定

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

* 禁止 `unsafe_code`、`unwrap`、`expect`、`panic!`、`as`(数值强转)、wildcard import(`use foo::*`)。需要数值转换时用 `TryFrom` / `try_into`;需要快速失败时用 `?` + `color_eyre::eyre::WrapErr`(`.wrap_err(...)` / `.wrap_err_with(...)`,trait 也兼容 `.context(...)` / `.with_context(...)`)。
* 错误处理统一走 **color-eyre**:报错宏用 `color_eyre::eyre::eyre!` / `bail!`,Report 类型是 `color_eyre::Report`。**不要再引入 `anyhow`**。channel 层错误仍是 `mineral_channel_core::Error`(`Other` 变体内含 `color_eyre::Report`),边界处用 `.map_err(Error::Other)` 收敛。
* **不要 `use` 任何 `Result` 类型**。要么定义命名 `type ToolResult = ...`,要么签名直接写 `-> color_eyre::Result<T>` / `-> mineral_channel_core::Result<T>`。
* 在测试函数里用 `?` + context 直接返回 `color_eyre::Result<()>`,业务断言用 `assert!` / `assert_eq!`,**不要 `unwrap`**。
* 对不透明字面量参数(`None` / `true` / `false` / 数字)写 `/*param_name*/` 注释,名字必须与函数签名完全一致;字符/字符串字面量不需要。
* 类型标注优先 turbofish:写 `Vec::<T>::new()`、`.collect::<Vec<T>>()`,而不是左侧 `: Vec<T>`。例外:trait object 向上转型(`let x: Arc<dyn Trait> = ...`)和无法推断的 `None`(`let x: Option<T> = None`)。
* `format!` / `println!` 等参数尽量内联(clippy `uninlined_format_args`)。
* 优先方法引用而非简单闭包;`match` 尽量穷尽,避免随手写 `_ =>`。

### 文档与可见性

* 所有 `pub` 项必须有 `///` 文档,模块必须有 `//!`,`pub struct` 的每个字段都必须有 `///`. 模块层面的注释不要提到其他模块, 属于注释层面的耦合
* `mod.rs`,`lib.rs` 仅用于模块导出/组织,**不要在 `mod.rs`,`lib.rs` 里写逻辑**。
* 优先不要pub模块 + 显式 `pub use` 控制对外 API;不要顺手把内部 type 标 `pub`。
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
* **TUI 错误恢复顺序**:`mineral/src/main.rs` 先 `color_eyre::install()`,再进 TUI;`mineral-tui` 的 `Tui::enter` 会取走当前 panic hook 并链式包一层,先 `restore_terminal()` 再调 prev。**不要**在 main 或 TUI 内部再装"裸"的 panic hook 绕过这条链——否则 panic 时彩色报告会被 alternate screen 吞掉,或者终端 raw mode 不恢复出现乱码。Result 冒泡走 `Tui::Drop` 的 `restore_terminal()`,顺序自然正确。
* `cargo apitest` 会真打 `music.163.com`;离线环境会失败,这不是 bug。
* TUI 离线开发默认走 `cargo run -p mineral --features mock`;不带 feature 跑也能起来,但 `playlists` / `tracks_cache` 都是空。
* `.claude/hooks/check_file_size.py` 用大括号配平剔除 `#[cfg(test)] mod` 块,字符串字面量里出现 `{` / `}` 可能误判;真遇到再升级到 `syn` AST。
