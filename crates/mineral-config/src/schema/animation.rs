//! 动画段(挂在 `TuiConfig` 下):帧率基准 + 各转场/浮层/扫入时长 + 视图扫入风格。
//!
//! [`SweepStyle`] 与渲染层过渡风格语义对齐,但保持解耦——接线处做映射。

use serde::Deserialize;

/// 动画配置。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[derive(Clone, Debug, Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct AnimationConfig {
    /// 主循环帧间隔(毫秒,≈ 60fps);重绘 / 拉数据 / 推进动画统一这一节奏。
    frame_tick_ms: u64,

    /// 整屏转场动画时长(tick)。
    transition_ticks: u16,

    /// 侧栏曲目扫入动画时长(tick)。
    sweep_ticks: u16,

    /// 全屏进退动画时长(tick)。
    fullscreen_ticks: u16,

    /// 浮层进出动画时长(tick)。
    popup_anim_ticks: u16,

    /// toast 进出动画时长(tick)。
    toast_anim_ticks: u16,

    /// 侧栏曲目扫入风格。
    view_sweep: SweepStyle,
}

/// 侧栏视图扫入过渡风格。不依赖渲染 crate;接线处映射到具体实现。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum SweepStyle {
    /// 推入栈:曲目从右滑入,同时把歌单往左推走。
    Push,

    /// 覆盖滑入:歌单原地不动,曲目从右盖上。
    Cover,
}
