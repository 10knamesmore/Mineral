//! 一次按键解析出的用户意图([`Action`])及其参数类型。
//!
//! 纯数据,不含执行逻辑:keymap 表把和弦映射到本枚举,`App::dispatch` 是唯一执行点。

/// 一次按键解析出的用户意图。keymap 表把 [`KeyChord`] 映射到本枚举;
/// `App::dispatch` 是其唯一执行点。
///
/// 分两族:
/// - **视图动作**:依赖 TUI 本地态(选中 / 视图 / 搜索 / 全屏 / 浮层),进程内执行。
/// - **领域动作**:转发为 [`mineral_server::Client`] 命令;执行点按下时从 `AppState`
///   解出具体目标(如选中歌)。Action 本身只带「不依赖运行期状态」的参数(步长等)。
///
/// 不持有 song_id 之类运行期句柄:那是 dispatch 时从选中行解析的,表项保持纯静态绑定,
/// 为后续 config 声明式重映射(default.lua / 用户 lua)留缝。
///
/// [`KeyChord`]: mineral_config::keys::KeyChord
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    // ---- 视图动作(TUI 本地) ----
    /// 进 / 退全屏播放态(toggle)。
    ToggleFullscreen,

    /// 进 / 退 Search 布局态(toggle;浏览态可达,全屏态屏蔽——两个全屏级布局态互斥)。
    OpenSearchView,

    /// 打开浮动播放队列(光标定位到在播歌)。
    OpenQueue,

    /// 打开退出确认浮层。
    OpenQuitConfirm,

    /// 打开键位 cheatsheet 浮层(已开时再按 = 关闭)。
    OpenHelp,

    /// 循环歌词副语言(原文 → 翻译 → 罗马音)。
    CycleLyricExtra,

    /// 滚动(逐行 / 翻页档行数见 `behavior.line_scroll_rows` / `page_scroll_rows`),
    /// 按上下文路由:全屏滚歌词,浏览态滚列表视口(光标与视口同移),queue 浮层滚队列。
    Scroll(ScrollStep),

    /// 进入搜索输入态(全屏态屏蔽)。
    EnterSearch,

    /// 列表光标移动(j/k/J/K/g/G 归一);全屏态屏蔽。
    MoveSelection(SelectionMove),

    /// 在当前视图「进入」(Playlists→Library / Library→播放选中曲)。
    ActivateSelection,

    /// 在当前视图「返回」(Library→Playlists;搜索非空时先清搜索)。
    BackOrClearSearch,

    /// 下探一层 / 进入选中项详情:搜索面板 song 进其所属专辑、容器进详情,歌手专辑区下钻该专辑。
    /// 与 [`Self::ActivateSelection`] 的区别在 song——activate 播放、本动作进专辑。
    DrillIntoSelection,

    /// 切换详情面板内分区(歌手:热门曲 ↔ 专辑)。
    CycleDetailSection,

    // ---- 领域动作(转 Client) ----
    /// 暂停 / 恢复(有当前曲才动)。
    TogglePlayPause,

    /// 循环播放模式(`m`)。
    CyclePlayMode,

    /// 音量增减,delta 为百分点(`+` / `-`)。
    NudgeVolume(VolumeDelta),

    /// 相对 seek,秒数可负(含 Shift 大跨)。
    SeekRelative(SeekDelta),

    /// 上一首 / 回开头(`p`)。
    PrevOrRestart,

    /// 下一首(`n`)。
    NextSong,

    /// 切换选中曲的 ♥(乐观翻转 + 转发)。
    ToggleLoveSelection,

    /// 下载当前视图选中项(Library→单曲 / Playlists→整张歌单)。
    DownloadSelection,

    /// 关最早一张驻留通知卡片(连按逐条关;无卡时空操作)。
    DismissNotice,

    /// 上下文操作菜单(内容随光标实体 × 视图;无实体时空操作)。
    OpenActionMenu,

    /// 复制菜单(内置项 + 自定义模板;无实体时空操作)。
    OpenCopyMenu,

    /// 触发脚本具名动作(`tui.keys.script` 绑定)。槽位经
    /// `Keymap::script_action` 解析回注册名(Action 须 `Copy`,名字不内嵌)。
    InvokeScript(ScriptSlot),
}

/// 列表光标的一次移动。归一 j/k(±1)与 J/K(大跨)与 g/G(首 / 末)。
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

/// 滚动的方向 + 档位。每档行数在执行点从 `behavior` 配置取(逐行档 `line_scroll_rows`、
/// 翻页档 `page_scroll_rows`),不内嵌枚举。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollStep {
    /// 逐行上滚。
    LineUp,

    /// 逐行下滚。
    LineDown,

    /// 翻页上滚。
    PageUp,

    /// 翻页下滚。
    PageDown,
}

/// 脚本动作槽位:`Keymap` 内 `script_names` 表的索引。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScriptSlot(pub usize);

/// 音量增量(百分点;可负)。newtype 避免 dispatch 出现裸 `i16` 谜语参数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VolumeDelta(pub i16);

/// seek 增量(秒;可负)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeekDelta(pub i64);
