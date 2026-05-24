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
cargo t                                           # = nextest run --workspace(跑全仓测试,见下 alias)
cargo td                                          # = test --workspace --doc(nextest 不跑 doctest,单独兜)
cargo snap                                        # = insta test --test-runner nextest --review(改了快照时)
cargo nextest run -p mineral-channel-netease      # 仅跑网易云 crate 的测试
cargo test --test crypto_vectors                  # 跑加密自检 harness(byte-for-byte 比对 openssl)
cargo nextest run -E 'test(parses_song_list)'     # 按测试名过滤(nextest filterset)
cargo fmt                                         # 格式化(rustfmt)
cargo fmt --check                                 # CI 用,不修改
cargo clippy --workspace --all-targets -- -D warnings  # 严格 lint(包括函数体量约束)
```

测试运行器是 **cargo-nextest**(需 `cargo install cargo-nextest cargo-insta`);`cargo t` / `td` / `snap` 是 `.cargo/config.toml` 里的 alias。nextest 配置见 `.config/nextest.toml`(daemon e2e 串行组 + retries)。nextest **不跑 doctest**,doctest 单独走 `cargo td`。

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
* **日志记错误统一用 `error = mineral_log::chain(&e)`**(内部 `format!("{e:#}")`):展开完整 context 链、单行、无 ANSI / backtrace。**不要**用 `error = %e`(Display 只给最外层一条,`.wrap_err()` 加的 context 全丢)或 `error = ?e`(color-eyre 的 Debug 带 ANSI 色码 + `Location` + `Backtrace`,污染纯文本日志文件)。错误想精确到字段时,在反序列化等边界用带路径的包装(如 netease `wire::de::from_value` 走 `serde_path_to_error`),让路径进入 message,`chain` 自然带出。
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
- 任何时候涉及到ipc, 原则是**rust内部一定优先结构化, 不要String, 在边缘适配层再序列化**

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

## 测试约定

### 用什么测什么

* **逻辑 / 算法 / 等价性** → 普通 `assert_eq!` / `assert!`。`next_in_queue`、`layout::compute`、`color` 数学、codec round-trip 这类,期望值写在断言里**表意最清晰**,**不要**为了统一上快照。
* **渲染 / 结构化输出** → **`insta` 快照**:
  - TUI 组件渲染:`ratatui::backend::TestBackend` 渲进内存 + `insta::assert_snapshot!`(**不依赖真 pty**,参考 `mineral-tui` 各 `components/*` 的 `#[cfg(test)]`)。
  - 解析层(netease `wire`、`mineral-model` LRC 等"输入→结构体"):`insta::assert_debug_snapshot!(parsed)`,一次锁整个结构,字段增删/默认值变化全抓得到。
* **性质 / 不变量(有清晰约束的纯函数)** → **`proptest` 属性测试**。声明"对任意输入恒成立"的性质,框架随机生成数百用例 + 失败自动收缩到最小反例。甜区:codec 编解码往返(`crates/mineral-protocol/tests/codec.rs` 的 `arb_request` round-trip)、解析器抗崩 + 自洽(`mineral-model` LRC:不 panic / 严出幂等)、`color` lerp 范围 / `layout::compute` 子区域不越界。失败种子存 `proptest-regressions/*.txt`,**进 git** 当永久回归用例(`color::lerp_byte` 的 u64 溢出就是这么钉住的)。**不适合**渲染 / IO / "输出就该长这样"——那些走 insta。
* **CLI 黑盒 e2e** → **`assert_cmd` + `predicates`**:`Command::cargo_bin("mineral")?...assert().success().stdout(predicate::str::contains(...))`(见 `crates/mineral/tests/cli_smoke.rs`)。
* **进程级 e2e** → `CARGO_BIN_EXE_<bin>` 起真二进制(见 `crates/mineral/tests/daemon_lifecycle.rs` 的 `Daemon` 框架:隔离 XDG + 子进程 + 读日志),覆盖 daemon 生命周期 / CLI 退出码 / socket 等。daemon 是**单 client**设计,测试里别用"探测连接 + 紧接真请求"两段式(busy 竞态);把就绪探测并进真请求的重试循环。

### 共享测试库 `mineral-test`

跨 crate 复用的测试零件收口在 **`mineral-test`** crate(普通 pub 库,各 crate 挂 **dev-dependency**):

* `song(id)` + 函数式装饰 `with_artist` / `with_name` / `with_source` / `with_duration`(造 `Song`,不要再各 crate 各抄一份)。
* 展示性 fixtures `endserenading(n)` / `chinese_football(n)`(真实曲目,CJK 测试用)。
* 快照断言宏 `assert_snap!` / `assert_snap_debug!`(强制中文 `description`)。
* proptest 生成器 `arb_song()`。

**只放跨 crate 能复用的(纯 `mineral-model` 类型)**;依赖某 crate 私有类型的 fixture(如 TUI 的 `state_with_*` 依赖 `AppState` / `SongView`)留在该 crate 自己的 `#[cfg(test)] mod test_support`,用 `pub(crate) use mineral_test::…` re-export 共享零件。`mineral-test` 依赖 `mineral-model`,而 `mineral-model` 测试又 dev-dep `mineral-test`——**经 dev-dependency 的依赖环 Cargo 允许**(dev-dep 不进正常构建图)。

### insta 用法约定

* **所有快照断言必须带中文 `description`**,`cargo insta review` 时逐张显示、便于辨认。统一走 `mineral_test::assert_snap!("…", backend)`(Display)/ `assert_snap_debug!("…", parsed)`(Debug)——这两个宏内置 description,**不要**再手写裸 `insta::with_settings!{description}{…}`。需要 `filters` 等额外设置时才直接用 `with_settings!`。
* **动态内容**(版本号 `mineral vX.Y.Z`、时间戳、UUID、临时路径等)用 `with_settings!({ filters => vec![(正则, "占位符")] }, …)` 归一化,否则每次变动都假失败。需要 workspace `insta` 开 `features = ["filters"]`(已开)。
* **`.snap` 文件进 git**。新增/改动渲染后 `cargo insta review` **逐张人工确认**再接受;**严禁** `INSTA_UPDATE=always` 盲接受。CI 必须 `INSTA_UPDATE=no`(或 `cargo insta test --unreferenced=reject`)防止漏审 / 未提交快照蒙混过关。
* HashMap 顺序不稳定用 `with_settings!({ sort_maps => true }, …)`;但 **ratatui `Table` 的 flex 列宽(`Constraint::Min` 有 slack 时)求解器本身非确定**,insta 治不了——这类表格快照会 flaky,得在生产侧改成确定性列约束(`sidebar/playlists` 表格就有这个潜在列宽闪烁 bug)。

### 测试也守 workspace lints

测试**不豁免** `Cargo.toml` 的 deny:无 `unwrap` / `expect` / `indexing_slicing`(用 `?` + `assert_*` + `.get()`),测试函数 / helper / 结构体字段一样要 `///`(`missing_docs_in_private_items`)。`#[cfg(test)] mod` 块不计入单文件 800 行上限。

### CI / 本地 git hooks

* **CI**(`.github/workflows/ci.yml`,PR + push main 触发,`main` 的 required check):装 `libasound2-dev`(alsa-sys **编译期**依赖,缺它 build 直接挂)+ `cargo-nextest` → `cargo fmt --all --check` → `cargo clippy --workspace --all-targets -- -D warnings` → `cargo nextest run --workspace`(`INSTA_UPDATE=no`)→ `cargo test --workspace --doc`。**不需要**真音频:audio engine 拿不到设备会降级 null 模式(见下「容易踩的点」),headless 照常起 daemon。
* **本地 git hooks**(`.githooks/`,走 `core.hooksPath`):**pre-commit** 只 `cargo fmt --all --check`(秒级);**pre-push** 跑 `clippy -D warnings` + `cargo nextest run` + `cargo test --doc`。新 clone 后启用一次:`git config core.hooksPath .githooks`。
* **Claude hooks**(`.claude/settings.json`):PostToolUse 跑 `check_file_size.py`(文件 ≤ 800 行);Stop 时 `cargo fmt`。

## 一些容易踩的点

* **不要为某个 channel 的特殊字段污染 `mineral-model`**——平铺合并是核心契约。
* **改动 `crypto/` 后必跑 `cargo test --test crypto_vectors`**,服务端解不出来不会立刻爆,而是返回 `code != 200` 的 JSON,排查成本高。
* **TUI 错误恢复顺序**:`mineral/src/main.rs` 先 `color_eyre::install()`,再进 TUI;`mineral-tui` 的 `Tui::enter` 会取走当前 panic hook 并链式包一层,先 `restore_terminal()` 再调 prev。**不要**在 main 或 TUI 内部再装"裸"的 panic hook 绕过这条链——否则 panic 时彩色报告会被 alternate screen 吞掉,或者终端 raw mode 不恢复出现乱码。Result 冒泡走 `Tui::Drop` 的 `restore_terminal()`,顺序自然正确。
* **音频无设备会降级 null 模式,不报错退出**:`mineral-audio` engine 拿不到默认输出设备(headless / 无声卡)时不 `return Err`,而是 warn + 置 `AudioSnapshot.backend = AudioBackend::Null` + 空跑(接受命令但不发声),daemon 照常 bind / serve / graceful shutdown。client 据此提示(CLI `status` 打 `backend: null`、TUI 顶栏 `⚠ 无音频设备`)。测试用 `AudioMode::ForceNull` / `MINERAL_AUDIO_NULL=1` env 确定性复现。**注**:`libasound2-dev` 是**编译期**依赖(alsa-sys),降级只省运行期声卡,省不了它。
* `cargo apitest` 会真打 `music.163.com`;离线环境会失败,这不是 bug。
* TUI 离线开发默认走 `cargo run -p mineral --features mock`;不带 feature 跑也能起来,但 `playlists` / `tracks_cache` 都是空。
* `.claude/hooks/check_file_size.py` 用大括号配平剔除 `#[cfg(test)] mod` 块,字符串字面量里出现 `{` / `}` 可能误判;真遇到再升级到 `syn` AST。
