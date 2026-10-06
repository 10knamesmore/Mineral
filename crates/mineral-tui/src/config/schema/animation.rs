//! 动画段(挂在 `TuiConfig` 下):帧率、转场、播放栏反馈与视图扫入时长。
//!
//! [`SweepStyle`] / [`MenuReveal`] 与渲染层过渡风格语义对齐,但保持解耦——接线处做映射。

use mineral_config_macros::{config_section, lua_enum};
use serde::Deserialize;

/// 动画配置。
#[config_section]
pub struct AnimationConfig {
    /// 主循环帧间隔
    frame_tick_ms: u64,

    /// 整屏转场时长
    transition_ms: u32,

    /// 侧栏歌单 ↔ 曲目切换扫入动画时长
    sweep_ms: u32,

    /// 列表滚动时长
    list_scroll_ms: u32,

    /// 缩略图光标移动时长
    minimap_cursor_ms: u32,

    /// 全屏进退场时长
    fullscreen_ms: u32,

    /// 控制键按压反馈时长
    controls_press_ms: u32,

    /// 播放栏操作反馈
    transport: TransportFeedbackConfig,

    /// 全屏背景跟随延迟
    ambient_trail: AmbientTrailConfig,

    /// 浮层进退场时长
    popup_anim_ms: u32,

    /// 通知展开收起时长
    toast_anim_ms: u32,

    /// 窗口焦点变色时长
    focus_fade_ms: u32,

    /// 搜索焦点边框滑动时长，仅 slide 生效
    search_focus_morph_ms: u32,

    /// 待机唱片旋转周期
    vinyl_rev_ms: u32,

    /// 侧栏切换样式
    view_sweep: SweepStyle,

    /// 菜单进场样式
    menu_reveal: MenuReveal,

    /// 搜索焦点切换样式
    search_focus_transition: SearchFocusTransition,

    /// 加载动画帧；空数组隐藏字形
    spinner_frames: Vec<String>,

    /// 长标题滚动
    marquee: MarqueeConfig,
}

/// 播放栏操作反馈
#[config_section]
pub struct TransportFeedbackConfig {
    /// 音量百分比停留时长
    volume_hold_ms: u32,

    /// 播放模式文字停留时长
    mode_hold_ms: u32,

    /// 播放按钮停留时长
    controls_hold_ms: u32,

    /// 旧音量文字淡出时长
    volume_fade_out_ms: u32,

    /// 新音量文字淡入时长
    volume_fade_in_ms: u32,

    /// 播放模式文字显现时长
    mode_reveal_ms: u32,

    /// 播放模式区域缩放时长
    mode_resize_ms: u32,

    /// 播放按钮淡入淡出时长
    controls_fade_ms: u32,
}

/// 全屏背景跟随延迟
#[config_section]
pub struct AmbientTrailConfig {
    /// 进入全屏
    enter: TrailTimingConfig,

    /// 退出全屏
    exit: TrailTimingConfig,
}

/// 背景延迟及过渡时长
#[config_section]
pub struct TrailTimingConfig {
    /// 背景跟随延迟
    delay_ms: u32,

    /// 背景过渡时长
    ease_ms: u32,
}

/// 长标题滚动
#[config_section]
pub struct MarqueeConfig {
    /// 滚动方式
    mode: MarqueeMode,

    /// 每列滚动时长
    step_ms: u32,

    /// 滚动前停顿时长
    pause_ms: u32,

    /// 边缘渐暗入场时长；0 为关闭
    fade_ms: u32,

    /// 两侧渐暗宽度（列）
    fade_cols: u16,

    /// 循环滚动参数
    #[serde(rename = "loop")]
    loop_: MarqueeLoopConfig,

    /// 往返滚动参数
    bounce: MarqueeBounceConfig,
}

/// 循环滚动
#[config_section]
pub struct MarqueeLoopConfig {
    /// 首尾拼接分隔串
    gap: String,
}

/// 往返滚动
#[config_section]
pub struct MarqueeBounceConfig {
    /// 折返前停顿时长
    edge_pause_ms: u32,
}

/// 长标题滚动方式
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarqueeMode {
    /// 首尾循环滚动
    Loop,

    /// 来回往返滚动
    Bounce,

    /// 静态截断
    Off,
}

/// 搜索焦点切换样式
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchFocusTransition {
    /// 边框滑动
    Slide,

    /// 直接切换
    Instant,
}

/// 菜单进场样式
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum MenuReveal {
    /// 从锚边逐行展开
    Directional,

    /// 从锚点缩放展开
    Morph,
}

/// 侧栏切换样式
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SweepStyle {
    /// 曲目推走歌单
    Push,

    /// 曲目覆盖歌单
    Cover,
}
