# 测试约定

本仓库的测试细则。CLAUDE.md 只留摘要 + 指针,具体看这里。

> TUI 测试的**通用方法论**(TestBackend / 快照 / proptest / 集成测试三招)见个人 `tui` skill 的 `references/testing.md`;本文是 **mineral 仓库的具体落地**(真实 helper 名 + 文件路径)。

测试运行器是 **cargo-nextest**(`cargo install cargo-nextest cargo-insta`);`cargo t` / `td` / `snap` 是 `.cargo/config.toml` 的 alias。nextest **不跑 doctest**,doctest 单独走 `cargo td`。nextest 配置见 `.config/nextest.toml`(daemon e2e 串行组 + retries)。

## 用什么测什么

* **逻辑 / 算法 / 等价性** → 普通 `assert_eq!` / `assert!`。`next_in_queue`、`layout::compute`、`color` 数学、codec round-trip 这类,期望值写在断言里**表意最清晰**,**不要**为了统一上快照。
* **渲染 / 结构化输出** → **`insta` 快照**:
  - TUI 组件渲染:`ratatui::backend::TestBackend` 渲进内存 + `insta::assert_snapshot!`(**不依赖真 pty**,参考 `mineral-tui` 各 `components/*` 的 `#[cfg(test)]`)。
  - 解析层(netease `wire`、`mineral-model` LRC 等"输入→结构体"):`insta::assert_debug_snapshot!(parsed)`,一次锁整个结构,字段增删/默认值变化全抓得到。
* **性质 / 不变量(有清晰约束的纯函数)** → **`proptest` 属性测试**。声明"对任意输入恒成立"的性质,框架随机生成数百用例 + 失败自动收缩到最小反例。甜区:codec 编解码往返(`crates/mineral-protocol/tests/codec.rs` 的 `arb_request` round-trip)、解析器抗崩 + 自洽(`mineral-model` LRC:不 panic / 严出幂等)、`color` lerp 范围 / `layout::compute` 子区域不越界。失败种子存 `proptest-regressions/*.txt`,**进 git** 当永久回归用例(`color::lerp_byte` 的 u64 溢出就是这么钉住的)。**不适合**渲染 / IO / "输出就该长这样"——那些走 insta。
* **CLI 黑盒 e2e** → **`assert_cmd` + `predicates`**:`Command::cargo_bin("mineral")?...assert().success().stdout(predicate::str::contains(...))`(见 `crates/mineral/tests/cli_smoke.rs`)。
* **进程级 e2e** → `CARGO_BIN_EXE_<bin>` 起真二进制(见 `crates/mineral/tests/daemon_lifecycle.rs` 的 `Daemon` 框架:隔离 XDG + 子进程 + 读日志),覆盖 daemon 生命周期 / CLI 退出码 / socket 等。daemon 是**单 client**设计,测试里别用"探测连接 + 紧接真请求"两段式(busy 竞态);把就绪探测并进真请求的重试循环。
* **TUI 交互 / 事件循环(进程内集成)** → 真造 `App` + 喂真实事件 + 跨 tick 验证,见下「TUI 集成测试基建」。

## 共享测试库 `mineral-test`

跨 crate 复用的测试零件收口在 **`mineral-test`** crate(普通 pub 库,各 crate 挂 **dev-dependency**):

* `song(id)` + 函数式装饰 `with_artist` / `with_name` / `with_source` / `with_duration`(造 `Song`,不要再各 crate 各抄一份)。
* 展示性 fixtures `endserenading(n)` / `chinese_football(n)`(真实曲目,CJK 测试用)。
* 快照断言宏 `assert_snap!` / `assert_snap_debug!`(强制中文 `description`)。
* proptest 生成器 `arb_song()`。

**只放跨 crate 能复用的(纯 `mineral-model` 类型)**;依赖某 crate 私有类型的 fixture(如 TUI 的 `state_with_*` 依赖 `AppState` / `SongView`)留在该 crate 自己的 `#[cfg(test)] mod test_support`,用 `pub(crate) use mineral_test::…` re-export 共享零件。`mineral-test` 依赖 `mineral-model`,而 `mineral-model` 测试又 dev-dep `mineral-test`——**经 dev-dependency 的依赖环 Cargo 允许**(dev-dep 不进正常构建图)。

## TUI 集成测试基建

`mineral-tui` 是 client/server 架构(App 每帧从 server 拉 `PlayerSnapshot` 灌进本地镜像)。要测「按键 → 状态 → 跨 tick → 渲染」这条真实链路,基建已就位,**直接用**:

* **`mineral_tui::test_support::TestClient`**:no-op 实现 `mineral_server::Client`(读取类返 `Default`、命令类静默吞),不接真 daemon / server。
* **`mineral_tui::test_support::app_with_queue(len, current_idx) -> App`**:接 `TestClient` + 禁用封面、填好 queue 的 `App`,**普通同步构造,不需 tokio runtime**。新场景照抄它再加 fixture。
* **`CoverFetcher::disabled()`**(`crates/mineral-tui/src/cover.rs`):不依赖 runtime 的 null object,让 `App::new` 脱离 cover/tokio 耦合。它也是**生产降级落点**(`spawn()` 失败时 `warn! + disabled()` 兜底,见 CLAUDE.md「容易踩的点」)。

写法:测试放 `app.rs` 等的 `#[cfg(test)] mod tests` 内(同模块可直接调私有 `handle_event` / `apply_player_snapshot` / `toggle_queue`):

* 喂键:`app.handle_event(&Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty())))`(`KeyEvent::new` 默认 `kind = Press`,正好过事件入口的 Press 过滤)。
* 模拟 server tick:手搓 `PlayerSnapshot`(全 pub 字段 + `..Default::default()`)调 `apply_player_snapshot`。
* 断言状态走 `assert_eq!`,视觉回归再补一张 `view::draw` / 组件 `draw` 的 insta 快照。

**这套专抓单元测试碰不到的时序 bug**:用户用按键改了 UI 状态、下一帧 tick 又从 server snapshot 覆盖回去(典型:UI 光标 vs server 的播放位置锚点共用字段)。参考 `app.rs` 的 `queue_nav_moves_and_survives_snapshot_tick` / `queue_cursor_decoupled_*`。

## insta 用法约定

* **所有快照断言必须带中文 `description`**,`cargo insta review` 时逐张显示、便于辨认。统一走 `mineral_test::assert_snap!("…", backend)`(Display)/ `assert_snap_debug!("…", parsed)`(Debug)——这两个宏内置 description,**不要**再手写裸 `insta::with_settings!{description}{…}`。需要 `filters` 等额外设置时才直接用 `with_settings!`。
* **动态内容**(时间戳、UUID、临时路径等)用 `with_settings!({ filters => vec![(正则, "占位符")] }, …)` 归一化,否则每次变动都假失败。需要 workspace `insta` 开 `features = ["filters"]`(已开)。版本号是例外:它在 `top_status.rs` 用 `#[cfg(test)]` 把 `DISPLAY_VERSION` 固定成占位值,**渲染源头**就不带真实版本,故含顶栏的快照走朴素 `assert_snap!` 即可、无需 per-test filter(避免新增顶栏快照时漏写 filter 又把版本号烤进去)。
* **`.snap` 文件进 git**。新增/改动渲染后 `cargo insta review` **逐张人工确认**再接受;**严禁** `INSTA_UPDATE=always` 盲接受。CI 必须 `INSTA_UPDATE=no`(或 `cargo insta test --unreferenced=reject`)防止漏审 / 未提交快照蒙混过关。
* HashMap 顺序不稳定用 `with_settings!({ sort_maps => true }, …)`;但 **ratatui `Table` 的 flex 列宽(`Constraint::Min` 有 slack 时)求解器本身非确定**,insta 治不了——这类表格快照会 flaky,得在生产侧改成确定性列约束(`sidebar/playlists` 表格就有这个潜在列宽闪烁 bug)。

## 测试也守 workspace lints

测试**不豁免** `Cargo.toml` 的 deny:无 `unwrap` / `expect` / `indexing_slicing`(用 `?` + `assert_*` + `.get()`),测试函数 / helper / 结构体字段一样要 `///`(`missing_docs_in_private_items`)。`#[cfg(test)] mod` 块不计入单文件 800 行上限。

## CI / 本地 git hooks

* **CI**(`.github/workflows/ci.yml`,PR + push main 触发,`main` 的 required check):装 `libasound2-dev`(alsa-sys **编译期**依赖,缺它 build 直接挂)+ `cargo-nextest` → `cargo fmt --all --check` → `cargo clippy --workspace --all-targets -- -D warnings` → `cargo nextest run --workspace`(`INSTA_UPDATE=no`)→ `cargo test --workspace --doc`。**不需要**真音频:audio engine 拿不到设备会降级 null 模式(见 CLAUDE.md「容易踩的点」),headless 照常起 daemon。
* **本地 git hooks**(`.githooks/`,走 `core.hooksPath`):**pre-commit** 只 `cargo fmt --all --check`(秒级);**pre-push** 跑 `clippy -D warnings` + `cargo nextest run` + `cargo test --doc`。新 clone 后启用一次:`git config core.hooksPath .githooks`。
* **Claude hooks**(`.claude/settings.json`):PostToolUse 跑 `check_file_size.py`(文件 ≤ 800 行);Stop 时 `cargo fmt`。
