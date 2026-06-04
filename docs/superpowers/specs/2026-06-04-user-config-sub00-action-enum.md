# Sub-00:Action 枚举统一(Phase 0 前置 TUI 重构)

> 父设计:[`2026-06-04-user-config-lua-design.md`](./2026-06-04-user-config-lua-design.md)(已定宪法)。
> 本 subspec 只细化「键位可配的先决工程」——把 `mineral-tui` 散落的键处理收敛成中央 `Action` 枚举 + 默认 keymap 表。
> 本文档按项目约定不纳入版本控制。

## 1. 范围与不做

### 做

- 抽出中央 `Action` 枚举(视图动作 / 领域动作两类),覆盖 `mineral-tui` 现有**所有**主视图与全局按键行为。
- 把现散落在 `handle_*_key()` 的 `match key.code` 收敛成**一张硬编码 keymap 表**(`(KeyChord) → Action`),`app.rs` 改成「查表得 Action → dispatch(action)」。
- 把浮层 `OverlayAction`(`popup/component.rs:79-88`)与主 `Action` 的关系厘清:**保留 `OverlayAction` 作为浮层→App 的回传意图,不并入 `Action`**(理由见 §4.3),但 `dispatch` 入口统一。
- 保留全部**上下文裁决逻辑**(搜索输入态、全屏屏蔽列表导航/搜索、转场吞键、Ctrl-C 强退、浮层优先吃键、半穿透),只改「键→行为」的中段,不动这些闸。
- 为 config 注入留一个**纯接口缝**:keymap 表来自一个可替换的 `Keymap` 值(本期硬编码 `Keymap::builtin()`),Phase 0 后续 PR 把它换成「default.lua + 用户 lua 解析出的表」即可,**本 subspec 不接任何 config / Lua**。

### 不做(交给相邻 subspec / 后续 Phase)

- 不引入 `mineral-config` / mlua / `default.lua`(Phase 0 的 config-loader subspec 负责)。
- 不引入 `KeyContext` / `KeyTriggered` 协议结构(`mineral-protocol` 现**无**这些类型,属 Phase 2「动作与生态」,见父设计 §7、§15)。
- 不引入「自定义动作名」转发到 daemon VM 的机制(Phase 2)。
- 不改键位的**默认绑定**(行为必须逐键不变,回归测试护航)。
- 不动渲染、不动 `Client` trait、不动 server。

边界一句话:**本 subspec 只重构 `mineral-tui` 进程内的「键 → 动作 → 执行」链路,产物是一个能被 config 表替换的 `Keymap`,但本期表的内容写死,行为零变化。**

## 2. 现状锚点(file:line)

键处理目前散落在 `crates/mineral-tui/src/app.rs`,无中央动作概念:

- `handle_event` — `app.rs:398`:crossterm `Event::Key` 按下边沿入口。
- `handle_key` — `app.rs:407`:顶层分发。内含全部上下文闸:
  - Ctrl-C 强退 — `app.rs:409-415`。
  - 转场进行中吞键 — `app.rs:419-421`。
  - 浮层优先吃键 + `OverlayResponse` 三态分发 — `app.rs:424-431`。
  - 搜索输入态路由 — `app.rs:434-437`。
  - `z` 全屏 toggle — `app.rs:441-444`;`Tab` 开 queue — `app.rs:447-451`;`q` 开 confirm — `app.rs:452-455`;`t` 切歌词副语言 — `app.rs:458-461`;`/` 进搜索(全屏屏蔽)— `app.rs:463-467`。
  - `handle_playback_key` 优先 — `app.rs:469-471`;全屏屏蔽列表导航后按 view 路由 — `app.rs:474-479`。
- `run_overlay_action` — `app.rs:493-506`:执行 `OverlayAction`(Quit / CloseTop / PlayQueueIndex)。
- `handle_overlay_passthrough` — `app.rs:509-515`:半穿透(`t` + playback)。
- `handle_search_key` — `app.rs:527-553`:搜索输入态按键(Esc/Enter/Backspace/Char)。
- `handle_playback_key` — `app.rs:556-582`:Shift+←→ 大跨 seek、空格/m/±/←→/p/n。
- `toggle_play_pause` / `nudge_volume` / `seek_relative` — `app.rs:585-618`:领域意图执行器(调 `self.client`)。
- `handle_playlists_key` — `app.rs:621-685`:j/k/J/K/g/G/d(下载歌单)/l|Enter(进 Library)/Esc(清搜索)。
- `handle_library_key` — `app.rs:688-757`:j/k/J/K/g/G/h|Esc|Backspace(回 Playlists)/Enter(setqueue+play)/f(love)/d(下载单曲)/Esc(清搜索)。

浮层侧动作体系(独立,与主视图不统一):

- `OverlayAction` 枚举 — `popup/component.rs:79-88`:`Quit` / `CloseTop` / `PlayQueueIndex(usize)`。
- `OverlayResponse` 枚举 — `popup/component.rs:66-75`:`Consumed` / `Pass` / `Do(OverlayAction)`。
- `QueueOverlay::on_key` — `popup/queue.rs:114-149`:浮层内 j/k/J/K/g/G/Enter/Tab|q|Esc + 其余 `Pass`。
- `ConfirmOverlay::on_key` — `popup/confirm.rs:67-74`;`DisconnectOverlay::on_key` — `popup/disconnect.rs:59-62`。

常量(键位语义参数,迁移时随表带走):`VOLUME_STEP` `app.rs:33`、`SEEK_STEP_S` `app.rs:36`、`SEEK_BIG_STEP_S` `app.rs:39`、`ROW_BIG_STEP` `app.rs:42`(及 `popup/queue.rs:19` 同名)。

支撑类型:`View` 枚举 — `runtime/state.rs:38`;选择/过滤 helper — `state.rs:402-448`(`current_selection` / `filtered_playlists` / `filtered_tracks` / `current_tracks` / `queue_current_index`)。`Client` trait — `mineral-server/src/client.rs:89`(领域动作的执行落点)。回归测试现状 — `app.rs:760-1150`(`press()` helper + `app_with_queue`/`app_with_library`/`app_in_fullscreen`)。

clippy 阈值:`too-many-lines-threshold = 300`(`clippy.toml`)。`handle_playlists_key`/`handle_library_key` 现各 ~65 行,迁移后查表分发的 `dispatch` 必须保持 < 300 行——靠**一个 action 一个小执行器方法**拆开。

## 3. 新增 / 修改文件清单

新增模块归到 `crates/mineral-tui/src/runtime/`(键位解析是运行时关注点,与 `state` / `view_model` 平级)。`runtime/mod.rs` 仅加 `pub mod` 声明,**不写逻辑**(`mod.rs:1-13` 现状)。

| 文件 | 职责 | 预估规模 |
|---|---|---|
| `runtime/action.rs`(新) | `Action` 枚举(视图动作 / 领域动作两族)+ 文档;`Action` 不含执行逻辑,纯数据。 | ~120 行 |
| `runtime/keymap.rs`(新) | `Keymap` 表类型 + `Keymap::builtin()`(硬编码默认绑定,逐键对齐现状)+ `lookup(chord) -> Option<Action>`。为 config 注入留 `Keymap::from_entries(...)` 构造缝。**`KeyChord` 类型(自有表示,不依赖 crossterm)+ `"space"/"Shift+Left"` 字符串解析器不在本文件,已裁决迁 `mineral-config::keys`(见 §10);本文件 `use mineral_config::keys::KeyChord`,crossterm `KeyEvent → KeyChord` 的转换在 `app.rs` 侧(见下)。** | ~220 行(含表;接近上限,见 §7 风险) |
| `runtime/mod.rs`(改) | 加 `pub mod action;` `pub mod keymap;` 两行。 | +2 行 |
| `app.rs`(改) | `handle_key` 改为「裁决闸 → 查表 → `dispatch(Action)`」;新增 `dispatch` + 每个 action 的小执行器;现 `handle_*_key` 删除或瘦身为执行器;Shift+seek 等并入表。**目标:文件不超 800 行**(现 1150 行含 ~390 行 `#[cfg(test)]`,非测试 ~760 行;迁移后非测试段须 ≤ 800,留意 `check_file_size.py` 配平)。 | 净增 ~30-60 行 |
| `popup/component.rs`(改) | `OverlayAction` 文档补一句与 `Action` 的关系;若 `PlayQueueIndex` 改为复用 `Action`,见 §4.3 决策——**默认不改**。 | ~0-10 行 |

若 `app.rs` 迁移后逼近 800 行(`check_file_size.py` 在 > 500 预警、> 800 阻断),把领域意图执行器(`toggle_play_pause`/`nudge_volume`/`seek_relative` 及新的 list-nav 执行器)整体抽到 `runtime/action.rs` 的一个 `impl` 块或新 `app/dispatch.rs` 子模块。**优先抽,不要把执行器堆回 `app.rs`。**

## 4. 关键类型与签名

### 4.1 `Action` 枚举(`runtime/action.rs`)

两族:**视图动作**(需 TUI 本地态,留在进程内)与**领域动作**(转 `Client` 命令,带显式参数)。参数用强类型——父设计「rust 内部一定优先结构化」。本期参数都是「按键瞬间从 `AppState` 解出的具体值」(如步长),**不在 Action 里塞 song_id**:song_id 是「按下时从选中行解析」的,放在 dispatch 执行器里查 `AppState`,Action 只表达「意图种类」。这样 keymap 表项是无状态的纯绑定,可被 config 静态声明。

```rust
/// 一次按键解析出的用户意图。keymap 表把 [`KeyChord`] 映射到本枚举;
/// [`App::dispatch`] 是其唯一执行点。
///
/// 分两族:
/// - **视图动作**:依赖 TUI 本地态(选中 / 视图 / 搜索 / 全屏 / 浮层),进程内执行。
/// - **领域动作**:转发为 [`mineral_server::Client`] 命令;执行点按下时从 [`AppState`]
///   解出具体目标(如选中歌)。Action 本身只带「不依赖运行期状态」的参数(步长等)。
///
/// 不持有 song_id 之类运行期句柄:那是 dispatch 时从选中行解析的,表项保持纯静态绑定,
/// 为后续 config 声明式重映射(default.lua / 用户 lua)留缝。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    // ---- 视图动作(TUI 本地) ----
    /// 进 / 退全屏播放态(toggle)。
    ToggleFullscreen,

    /// 打开浮动播放队列(光标定位到在播歌)。
    OpenQueue,

    /// 打开退出确认浮层。
    OpenQuitConfirm,

    /// 循环歌词副语言(原文 → 翻译 → 罗马音)。
    CycleLyricExtra,

    /// 进入搜索输入态(全屏态屏蔽)。
    EnterSearch,

    /// 列表光标移动(j/k/J/K/g/G 归一);全屏态屏蔽。
    MoveSelection(SelectionMove),

    /// 在当前视图「进入」(Playlists→Library / Library→播放选中曲)。
    ActivateSelection,

    /// 在当前视图「返回」(Library→Playlists;搜索非空时先清搜索)。
    BackOrClearSearch,

    // ---- 领域动作(转 Client) ----
    /// 暂停 / 恢复(有当前曲才动)。
    TogglePlayPause,

    /// 循环播放模式(`m`)。
    CyclePlayMode,

    /// 音量增减,`delta` 为百分点(`+` / `-`)。
    NudgeVolume(VolumeDelta),

    /// 相对 seek,`secs` 为秒(可负;含 Shift 大跨)。
    SeekRelative(SeekDelta),

    /// 上一首 / 回开头(`p`)。
    PrevOrRestart,

    /// 下一首(`n`)。
    NextSong,

    /// 切换选中曲的 ♥(乐观翻转 + 转发)。
    ToggleLoveSelection,

    /// 下载当前视图选中项(Library→单曲 / Playlists→整张歌单)。
    DownloadSelection,
}

/// 列表光标的一次移动。归一 j/k(±1)与 J/K(±[`ROW_BIG_STEP`])与 g/G(首 / 末)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionMove {
    /// 向下 `n` 行(钳到末行)。
    Down(usize),

    /// 向上 `n` 行(钳到首行)。
    Up(usize),

    /// 跳首行。
    First,

    /// 跳末行。
    Last,
}

/// 音量增量(百分点;可负)。newtype 避免 `dispatch` 出现裸 `i16` 谜语参数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VolumeDelta(pub i16);

/// seek 增量(秒;可负)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeekDelta(pub i64);
```

> 注:`VolumeDelta` / `SeekDelta` / `SelectionMove` 是**动作参数**,不是「对外配置 struct」——它们不被外部构造、不进 serde、无 builder 约定;字段 `pub` 仅供同 crate dispatch 读。对外配置 struct(私有字段 + `#[non_exhaustive]` + getters)的约定由 config-loader subspec 的 `Config` 承担,本枚举不触发它。

### 4.2 `Keymap`(`runtime/keymap.rs`)

> **已裁决(见 §10)**:归一化按键和弦 `KeyChord`(自有表示,不依赖 crossterm)+ `"space"/"Shift+Left"` 字符串解析器**归 `mineral-config::keys`**,由 sub00 的 PR-A 先落进 mineral-config(该 crate 现无 mlua 依赖,可先承载纯类型)。本 subspec **复用** `mineral_config::keys::KeyChord`,不在 `keymap.rs` 重定义。crossterm `KeyEvent → KeyChord` 的归一转换(丢弃无关修饰位、大小写已编码进 `Char`、仅保留 SHIFT / CONTROL)在 **mineral-tui 侧**(本 subspec 在 `app.rs` 或 `keymap.rs` 提供一个 `chord_from_event(&KeyEvent) -> KeyChord` 小工具,落点在 mineral-tui 因其依赖 crossterm)。`mineral_config::keys::KeyChord` 形如:

```rust
// crates/mineral-config/src/keys.rs(由 sub00 PR-A 落,sub01 keys schema 与 sub02 接线复用)
/// 归一化的按键和弦(自有表示,不依赖 crossterm):物理键 + 关心的修饰键
/// (只保留 SHIFT / CONTROL,过滤终端漂移的 KEYPAD / 大小写隐含 SHIFT 等噪声)。可作 HashMap key。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord { /* 自有 code + mods 表示 */ }

impl KeyChord {
    /// 解析键字符串(`"space"` / `"Shift+Left"` / `"S"`),非法返回 `Err`。
    pub fn parse(s: &str) -> color_eyre::Result<Self> { /* ... */ }
}
```

```rust
// crates/mineral-tui/src/runtime/keymap.rs —— 复用上面的 KeyChord
use mineral_config::keys::KeyChord;

/// 从一个 crossterm `KeyEvent` 归一到 [`KeyChord`](mineral-tui 侧,因依赖 crossterm)。
pub fn chord_from_event(key: &crossterm::event::KeyEvent) -> KeyChord { /* ... */ }

/// 键 → 动作绑定表。本期 [`Self::builtin`] 写死;Phase 0 后续 PR 用
/// [`Self::from_entries`] 喂入「default.lua + 用户 lua 解析出的绑定」。
pub struct Keymap {
    /// 归一和弦 → 动作。一对一(单动作);多键映同动作即多条目。
    table: FxHashMap<KeyChord, Action>,
}

impl Keymap {
    /// 内建默认绑定(逐键对齐重构前 `app.rs` 行为)。
    pub fn builtin() -> Self { /* ... */ }

    /// 从外部绑定构造(config 注入缝;本 subspec 不调用)。
    pub fn from_entries(entries: impl IntoIterator<Item = (KeyChord, Action)>) -> Self { /* ... */ }

    /// 查表:命中返回对应 [`Action`],未绑定返回 `None`。
    pub fn lookup(&self, chord: &KeyChord) -> Option<Action> {
        self.table.get(chord).copied()
    }
}
```

> `pub` 项均带 `///`;无 `unwrap`/`expect`/`panic`/`as`,`chord_from_event` 内不做数值强转。`builtin()` 用 `from_entries` 喂一组字面量绑定。

### 4.3 `OverlayAction` 处置策略(决策记录)

**保留 `OverlayAction` 不并入 `Action`。** 理由:

1. 浮层按键产出的是「浮层生命周期意图」(`CloseTop`)与「以浮层私有光标为参数的领域意图」(`PlayQueueIndex(usize)`),与主视图 keymap 的语义层级不同——浮层光标是 overlay 私有态(`queue.rs:25-28`),不在 `AppState`,无法用「dispatch 时查 AppState」的范式表达。
2. 浮层的键解析在 `*Overlay::on_key`(`queue.rs:114`),走 `OverlayResponse::{Consumed,Pass,Do}` 三态,本身已是干净的小型动作体系;强行并入主 `Action` 会让主 dispatch 背上浮层私有态依赖。
3. 父设计 §7 把浮层归「视图动作(滚动 / 全屏 / 浮层)→ TUI 本地执行」,未要求枚举合一。

**统一的是入口而非枚举**:`run_overlay_action`(`app.rs:493`)与新 `dispatch` 在 `App` 上是平级 dispatcher;`OverlayAction::Quit` 与 `Action` 无对应项(主视图无「直接退出」绑定,`q` 是开 confirm),`CloseTop`/`PlayQueueIndex` 也无主视图镜像。后续若 config 要让浮层键可配,再单独评估浮层 keymap(超出本 subspec)。

`popup/component.rs` 唯一改动:给 `OverlayAction` 文档补一句「浮层私有动作,不并入 `runtime::action::Action`,见 sub-00 §4.3」。

## 5. 实现步骤(依赖顺序 / 可拆 PR)

**PR-A:落地类型骨架(零行为改动)**

1. 新增 `runtime/action.rs`:`Action` + `SelectionMove` + `VolumeDelta` + `SeekDelta`,全 `///` 文档。
2. 新增 `runtime/keymap.rs`:`Keymap` + `builtin()`(逐键对照 §2 锚点填表)+ `lookup` + `chord_from_event`;`KeyChord` 由 `mineral-config::keys` 提供(本 PR-A 一并落 mineral-config 的 `keys` 模块——`KeyChord` 类型 + `parse` 字符串解析器)。
3. `runtime/mod.rs` 加两行 `pub mod`。
4. 单测:`keymap.rs` 内 `#[cfg(test)]` 断言 `builtin().lookup(chord)` 对每个现有键给出预期 `Action`(表正确性,纯函数,不起 App)。
5. 此 PR 不接 `app.rs`,`Action`/`Keymap` 暂时 `dead_code`——用 `#[allow(dead_code)]` 临时标注并在注释写明「PR-B 接通后移除」(对齐 `state.rs:61` 既有先例)。

**PR-B:`app.rs` 接管(行为不变迁移)**

6. `App` 持有 `keymap: Keymap` 字段(`App::new` 里 `Keymap::builtin()`;`#[cfg(test)]` fixture 不需改,走同一构造)。
7. 改 `handle_key`:**保留全部裁决闸不动**(Ctrl-C / 转场 / 浮层 / 搜索态),在「无活跃浮层、非搜索态」之后改为:`let chord = chord_from_event(key);` → `if let Some(action) = self.keymap.lookup(&chord) { self.dispatch(action); }`。
8. 新增 `App::dispatch(&mut self, action: Action)`:`match action` 调各执行器。**视图动作的上下文裁决在 dispatch 内保留**——例如 `MoveSelection`/`EnterSearch`/`ActivateSelection`/`BackOrClearSearch` 开头判 `self.state.fullscreen` 直接 return(替代现 `app.rs:463`/`474` 的 `!fullscreen` 闸),保证全屏屏蔽语义逐字不变。
9. 抽执行器:`move_selection(SelectionMove)`(合并 `handle_playlists_key`/`handle_library_key` 的 j/k/J/K/g/G 分支,按 `self.state.view` 分流)、`activate_selection()`(l/Enter 分流)、`back_or_clear_search()`(h/Esc/Backspace + 清搜索)、`download_selection()`、`toggle_love_selection()`。复用现成执行器 `toggle_play_pause`/`nudge_volume`/`seek_relative`(`app.rs:585-618`)。
10. 删除 `handle_playback_key`/`handle_playlists_key`/`handle_library_key` 的「键解析」部分(逻辑迁入 dispatch + 执行器);搜索态 `handle_search_key`(`app.rs:527`)**原样保留**(输入态不走 keymap,是字符累积,语义不同)。
11. 控制 `dispatch` 行数 < 300:它只做 `match` + 一行调用每分支;真正逻辑在执行器里。
12. 控制 `app.rs` 非测试段 < 800:若超,把执行器抽到 `runtime/action.rs` 的 `impl App`(需 `pub(crate)`)或新建 `app/dispatch.rs`。**先量再抽**(`check_file_size.py` 会在 Edit/Write 时报)。

**PR-B 的浮层侧**:`run_overlay_action`(`app.rs:493`)与 `OverlayAction` 保持不变;仅在 `popup/component.rs` 加 §4.3 那句文档。

> A/B 拆分让 review 聚焦:A 是纯增、可独立测表;B 是「删散落 match + 接 dispatch」,回归测试(§6)全程绿。若团队偏好单 PR,A+B 合并亦可——但 §6 测试必须在合并提交内全绿。

## 6. 测试清单(对齐 docs/testing.md)

测试运行器 nextest;真实 IO/engine 不涉及(`TestClient` 同步、无 tokio rt 需求,`app_with_queue` 注释已言明「同步构造,不需 tokio runtime」),故**本 subspec 测试无 multi_thread rt 要求**——但若后续在执行器里误引 daemon e2e,须并入 `.config/nextest.toml` 的 daemon 串行组并 `MINERAL_AUDIO_NULL=1`(此处不涉及)。

**A. keymap 表正确性(`keymap.rs` 单测,纯函数)**

- `builtin_maps_every_known_key`:逐键 `assert_eq!(km.lookup(&chord), Some(expected_action))`,覆盖 §2 列出的全部绑定(空格/m/+/=/−/_/←/→/Shift+←/Shift+→/p/n/j/k/J/K/g/G/l/Enter/h/Esc/Backspace/f/d/z/Tab/q/t/`/`)。
- `unbound_key_returns_none`:如 `KeyCode::Char('!')` → `None`。
- `chord_normalizes_modifiers`:同一逻辑键带无关修饰位时归一一致。
- 用 `assert_eq!` / `assert!`,**不 `unwrap`**(`chord_from_event` 不返回 Result;构造直接断言)。

**B. 行为不变回归(`app.rs` 的 `#[cfg(test)] mod`,既有 fixture 复用)**

现有测试(`app.rs:900-1149`)是天然的回归网,**重构后必须全部不改测试代码即绿**:

- `queue_nav_moves_and_survives_snapshot_tick`(`:903`)— 浮层路径未变,应原样绿。
- `tab_opens_queue_esc_closes`(`:943`)、`quit_plays_shrink_animation_then_exits`(`:964`)— Tab/q 经 keymap 命中 `OpenQueue`/`OpenQuitConfirm`,行为不变。
- `ctrl_c_exits_immediately_without_animation`(`:1004`)— Ctrl-C 闸在 keymap **之前**,必须仍立即退。
- `search_backspace_on_empty_query_exits`(`:1017`)— `/` 经 keymap 命中 `EnterSearch`,之后 `handle_search_key` 原样;vim 退格语义不变。
- `pressing_f_toggles_loved_optimistically`(`:1044`)— `f` → `ToggleLoveSelection` 执行器,乐观翻转不变。
- `z_toggles_fullscreen`(`:1104`)、`fullscreen_blocks_nav_and_search`(`:1120`)、`fullscreen_tab_still_opens_queue`(`:1137`)— 全屏裁决迁入 dispatch 后仍逐字不变(关键断言:`j`/`g`/`/` 在全屏被吞、`Tab` 仍开 queue)。

**新增回归(补 §2 中现有测试未覆盖的键,经 `press()` + `app_with_*`)**:

- `j_k_navigate_in_playlists_and_library`:`app_with_library` 下 j/k/J/K/g/G 移动 `sel_track`,断言落点(覆盖 `MoveSelection` 各 variant)。
- `volume_and_seek_via_keymap`:`+`/`-`/←/→/Shift+←/Shift+→ 经 dispatch 调到 `TestClient`(`TestClient::set_volume` 是 no-op,断 `app.state.playback.volume_pct` 本地乐观值变化,对齐 `nudge_volume` 现行为 `app.rs:597-603`)。
- `l_enters_library_enter_plays`:Playlists 视图 `l`/Enter 切 `View::Library`;Library 视图 Enter 触发 `set_queue`+`play_song`(`TestClient` no-op,断 `view` / 不 panic)。
- `d_downloads_selection_by_view`:Playlists `d` → 歌单下载意图、Library `d` → 单曲(`TestClient::download` no-op,断不 panic + 走对的分支可经轻量 spy client 或断 view 分流;若不引 spy,至少断「不改选中、不 panic」)。

**C. 快照(可选,低优先)**

本 subspec 不改渲染,**无需新增 `assert_snap!`**。`config check` 输出快照属 config-loader subspec。若想给「默认 keymap 表」一个人读快照,可在 `keymap.rs` 加一个 `assert_snap!("默认键位绑定表", debug_dump)` ——**带中文 description**、走 `mineral_test::assert_snap!`、`.snap` 进 git、`cargo insta review` 人工确认(禁 `INSTA_UPDATE=always`)。非必须。

## 7. 验收判据

1. `cargo build -p mineral-tui` / `cargo build -p mineral --features mock` 绿。
2. `cargo clippy --workspace --all-targets -- -D warnings` 绿(尤其 `too_many_lines`:`dispatch` < 300 行;无 `wildcard_imports`/`needless_pass_by_value`/`unwrap_used` 等)。
3. `cargo t`(全仓 nextest)绿:§6.A 表测试 + §6.B 全部既有回归**未改测试代码**通过 + §6.B 新增回归通过。
4. `check_file_size.py` 不阻断:`app.rs` 非测试段 ≤ 800、新文件 ≤ 800、`dispatch`/执行器各函数 ≤ 300。
5. 手验(`cargo run -p mineral --features mock`):逐键过一遍 §2 锚点列出的所有键,行为与重构前肉眼一致(全屏屏蔽、搜索态、浮层半穿透、Shift 大跨 seek)。
6. `Keymap::from_entries` 存在且可被外部 crate 调用编译通过(注入缝就绪),但本 subspec 无调用方——`#[allow(dead_code)]` 或加一条单测占用即可。

## 8. 风险

| 风险 | 缓解 |
|---|---|
| `app.rs` 现 1150 行,迁移后非测试段逼近 800 触发 hook 阻断 | §3 已规划:执行器抽到 `runtime/action.rs` 的 `impl App` 或新 `app/dispatch.rs`;**先量再抽**,不堆回 `app.rs`。 |
| `keymap.rs` 含表 + 测试逼近上限 | 表用 `from_entries(字面量数组)` 紧凑表达;表测试若臃肿,拆到 `keymap.rs` 的 `#[cfg(test)] mod`(不计 800 行)。 |
| 上下文裁决(全屏屏蔽 / 搜索态)迁移时漏一处导致行为漂移 | §6.B 既有回归测试 `fullscreen_blocks_nav_and_search` / `search_backspace_*` / `fullscreen_tab_still_opens_queue` 直接守;**这些测试不许改**,改了即说明行为变了。 |
| `KeyChord` 归一把 crossterm 修饰位过滤错(如 `+`/`=`、`Shift+方向` 被吞) | `chord_from_event`(mineral-tui 侧)只保留 SHIFT/CONTROL;现状 `handle_playback_key`(`app.rs:557`)用 `modifiers.contains(SHIFT)` 判 Shift+←→,且 `+`/`=`、`-`/`_` 是同 Action 的多键绑定——`builtin()` 表里逐键列全;§6.A `volume_and_seek_via_keymap` 直接覆盖。 |
| 多键映同动作(`+`=`、`-`/`_`、j/Down、h/Esc/Backspace)在「键→动作」表里展开成多条目,易漏 | `builtin()` 每个别名一条 `(chord, action)`;§6.A 逐键断言。 |
| `OverlayAction` 不并入 `Action` 被误读为「没统一」 | §4.3 明确「统一的是 dispatch 入口与动作概念,非枚举合一」;浮层私有光标态决定了不能合并。 |
| PR-A 的 `dead_code` 临时 allow 忘删 | PR-B 接通后移除;验收判据 2(clippy `-D warnings`)会在 PR-B 把残留 `dead_code` 暴露(若 `Keymap`/`Action` 全部接通则无 dead_code)。 |

## 9. 与相邻 subspec 的接口

- **config-loader subspec(Phase 0)**:本 subspec 产出的 `Keymap::from_entries(impl IntoIterator<Item = (KeyChord, Action)>)` 是注入点;config 侧负责把「default.lua + 用户 lua 的 keys 表」解析成 `Vec<(KeyChord, Action)>`(动作名字符串 → `Action` 的映射在 config 侧;**键字符串 → `KeyChord` 的解析已裁决归 `mineral-config::keys`**,由本 subspec PR-A 先落、sub01 keys schema 与 sub02 接线复用同一 `KeyChord` 与解析器,见 §10)。本 subspec 只保证 `Action` 是 `pub` + `Copy` + 文档齐全、`from_entries` 可外部调用。
- **theme-from-config subspec**:无交集(本 subspec 不碰 `Theme` / 渲染)。
- **Phase 2 daemon-VM / KeyContext subspec**:本 subspec 的 `Action` 只含「内建动作」;Phase 2 增「自定义动作名 → 转发 daemon」时,在 dispatch 的 `lookup` miss 分支或 `Action` 旁加一条 `Action::Custom(ActionName)` 透传——**本期不预留该 variant**(YAGNI,避免空壳)。

## 10. 开放问题(已裁决)

- **PR-A / PR-B 拆分**:**已裁决** ✔ 按 §5 拆——PR-A 纯增类型骨架(可独立测表),PR-B 接 `app.rs`(删散落 match + 接 dispatch,回归测试全程绿)。
- **默认 keymap 表人读快照**:**已裁决** ✔ 列入 PR-A——给「默认键位绑定表」加一个 `assert_snap!`(走 `mineral_test::assert_snap!`、带中文 `description`、`.snap` 进 git、`cargo insta review` 人工确认,禁 `INSTA_UPDATE=always`)。即 §6.C 的"可选快照"提升为 PR-A 必做项。
- **键字符串解析归属**:**已裁决** ✔ `KeyChord` 类型 + `"space"/"Shift+Left"` 字符串解析器放 **`mineral-config::keys`**(自有表示,不依赖 crossterm;mineral-config 现无 mlua 依赖也可先承载纯类型),由本 subspec PR-A 先落;mineral-tui 负责 crossterm `KeyEvent → KeyChord` 转换(`chord_from_event`);sub01 的 keys schema 与 sub02 接线复用同一类型。详见 §3 表 / §4.2 / §9。
- **spy Client**:**已裁决** ✔ 本期**不**新增 spy Client。§6.B 涉及领域动作分流的回归(如 `d_downloads_selection_by_view`)只断言「不 panic + 选中/视图不变」即可,不验 Client 调用细节。
