//! 组件共享的本帧配置与准备能力；业务数据由各组件独立声明。

use crate::image::{ImageNeeds, ImageRenderPhase};
use crate::render::theme::Theme;
use crate::runtime::scroll::list::ScrollMotion;
use std::time::Instant;

/// 本次更新所用的只读配置、主题和显式采样时间。
#[derive(Clone, Copy)]
pub(crate) struct FrameEnv<'a> {
    /// 当前有效配置，热更新后替换借用，不在组件构造时复制。
    pub(crate) config: &'a mineral_config::Config,

    /// 本次有效主题。
    pub(crate) theme: &'a Theme,

    /// 主循环明确采样的时刻。
    pub(crate) now: Instant,
}

/// 列表在布局已知后更新自己的状态，并向资源管线声明需求。
pub(crate) struct PrepareCx<'a> {
    /// 本次显示环境。
    pub(crate) frame: FrameEnv<'a>,

    /// 组件能够声明图片需求，不能直接调度 worker 或提交终端协议。
    pub(crate) images: ImageNeeds<'a>,

    /// 当前布局是否允许更新滚动目标。
    pub(crate) motion: ScrollMotion,

    /// 图片所处的几何和滚动阶段。
    pub(crate) image_phase: ImageRenderPhase,

    /// 本次是否走过一个逻辑时钟节拍。
    pub(crate) advance: bool,
}

impl FrameEnv<'_> {
    /// 按当前配置折算位置动画拍数，不保存配置副本。
    pub(crate) fn cursor_ticks(self) -> u16 {
        let anim = self.config.tui().animation();
        crate::render::anim::ticks16_from_ms(*anim.minimap_cursor_ms(), *anim.frame_tick_ms())
    }
}
