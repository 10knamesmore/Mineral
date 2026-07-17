//! 动画段(挂在 `TuiConfig` 下):帧率基准 + 各转场/浮层/扫入时长 + 视图扫入/菜单进场风格。
//!
//! [`SweepStyle`] / [`MenuReveal`] 与渲染层过渡风格语义对齐,但保持解耦——接线处做映射。

use mineral_config_macros::{config_section, lua_enum};
use serde::Deserialize;

/// 动画配置。
#[config_section]
pub struct AnimationConfig {
    /// 主循环帧间隔(毫秒;16 ≈ 60fps,越小越流畅越费 CPU);重绘 / 拉数据 / 推进动画统一这一节奏。
    /// 它是所有时长旋钮(`*_ms`)折算成拍数的分母——改它不改各动画的真实时长。
    frame_tick_ms: u64,

    /// 整屏转场(启动扩大 / 退出收缩)动画时长(毫秒)。
    transition_ms: u32,

    /// 侧栏歌单 ↔ 曲目切换扫入动画时长(毫秒)。
    sweep_ms: u32,

    /// 列表视口滚动平移时长(毫秒;逐行 / 翻页滚动与 scrolloff 触发的滚动共用)。
    list_scroll_ms: u32,

    /// 全屏播放态进退场形变动画时长(毫秒)。
    fullscreen_ms: u32,

    /// 浮层(队列 / 确认框)进出动画时长(毫秒)。
    popup_anim_ms: u32,

    /// toast(顶栏通知)横向展开收起动画时长(毫秒)。
    toast_anim_ms: u32,

    /// 终端失焦/聚焦时顶栏变灰的淡入淡出时长(毫秒)。
    focus_fade_ms: u32,

    /// Search 布局态焦点高亮边框滑动时长(毫秒);仅 `search_focus_transition = Slide` 时生效。
    search_focus_morph_ms: u32,

    /// 待机(无在播曲)唱片纹封面高光旋转一整圈的时长(毫秒)。
    vinyl_rev_ms: u32,

    /// 侧栏曲目扫入风格。
    view_sweep: SweepStyle,

    /// 锚定弹出菜单(PopMenu)的进场风格。
    menu_reveal: MenuReveal,

    /// Search 布局态焦点高亮边框切换的过渡风格(直切 / 边框滑动)。
    search_focus_transition: SearchFocusTransition,

    /// loading 占位的旋转 spinner 帧(逐帧循环画;search「searching」/ detail 数据未到共用)。
    /// 默认 braille 一周;每帧按 `frame_tick_ms` 节奏推进。空数组 = 不画字形(仅留 loading 文案)。
    spinner_frames: Vec<String>,

    /// 溢出标题滚动(marquee)段(选中行 / 播放栏长歌名)。
    marquee: MarqueeConfig,
}

/// 溢出标题滚动(marquee)配置。
#[config_section]
pub struct MarqueeConfig {
    /// 滚动方式(循环 / 来回往返 / 关闭)。
    mode: MarqueeMode,

    /// 每前进 1 列的毫秒;越小滚越快。
    step_ms: u32,

    /// 起步 / 选中切换后的停顿毫秒(先读到开头再开滚)。
    pause_ms: u32,

    /// 滚动窗口边缘 fade 的渐入毫秒(相位重置起缓升到满强度);0 = 关闭边缘 fade。
    fade_ms: u32,

    /// 边缘 fade 的空间宽度(列):窗口两侧各这么多列内逐级变暗。
    fade_cols: u16,

    /// 循环方式(`mode = "loop"`)独有段。
    #[serde(rename = "loop")]
    loop_: MarqueeLoopConfig,

    /// 往返方式(`mode = "bounce"`)独有段。
    bounce: MarqueeBounceConfig,
}

/// 溢出标题循环滚动(`mode = "loop"`)独有配置。
#[config_section]
pub struct MarqueeLoopConfig {
    /// 循环拼接处的分隔串;空串 = 首尾直接相接。
    gap: String,
}

/// 溢出标题往返滚动(`mode = "bounce"`)独有配置。
#[config_section]
pub struct MarqueeBounceConfig {
    /// 到达两端后的停顿毫秒(读完首 / 尾再折返);0 = 不停顿直接折返。
    edge_pause_ms: u32,
}

/// 溢出标题的滚动方式。不依赖渲染 crate;接线处映射到具体实现。
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarqueeMode {
    /// 循环滚动:文本首尾相接(中间夹 `gap`)向左匀速循环。
    Loop,

    /// 来回往返:滚到末尾后反向滚回开头,不拼接 `gap`。
    Bounce,

    /// 关闭:溢出标题维持静态截断,不滚动。
    Off,
}

/// Search 布局态焦点高亮边框切换的过渡风格。不依赖渲染 crate;接线处映射到具体实现。
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchFocusTransition {
    /// 边框滑动:高亮边框从旧面板矩形几何插值滑到新面板矩形。
    Slide,

    /// 直切:高亮边框瞬移到新面板,无过渡。
    Instant,
}

/// 锚定弹出菜单进场风格。不依赖渲染 crate;接线处映射到具体实现。
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum MenuReveal {
    /// 方向性揭开:宽度恒满,贴锚边自上而下(或镜像)生长。
    Directional,

    /// 形变:从锚点行矩形几何插值成最终菜单矩形,内容随重叠区揭入。
    Morph,
}

/// 侧栏视图扫入过渡风格。不依赖渲染 crate;接线处映射到具体实现。
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SweepStyle {
    /// 推入栈:曲目从右滑入,同时把歌单往左推走。
    Push,

    /// 覆盖滑入:歌单原地不动,曲目从右盖上。
    Cover,
}
