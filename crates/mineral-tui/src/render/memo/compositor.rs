//! 保留上一帧画布及终端输出参照，把组件变化贯通到局部提交。

use ratatui::{Frame, buffer::Cell, layout::Rect};

use super::surface::Surface;
use super::{FramePlan, Placement};

/// 终端会话的合成历史；与 Terminal 一同创建、resize 和释放。
#[derive(Default)]
pub(crate) struct Compositor {
    /// 最近合成的组件排列，用来恢复移走或卸载的区域。
    placements: Vec<Placement>,

    /// 终端画布的输出参照；尚未绘制时没有画布。
    surface: Option<Surface>,
}

/// 本次合成需要提交的 cell 和绘制工作量。
pub(crate) struct FrameUpdate {
    /// 实际运行绘制函数的组件数，不含缓存片段回填。
    pub(crate) painted_components: usize,

    /// 待提交 cell 在输出参照中的位置。
    indices: Vec<usize>,
}

impl FrameUpdate {
    /// 是否没有 cell 需要发送；图片指令由终端入口独立处理。
    pub(crate) fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// 本次实际更新的 cell 数，供提交日志使用。
    pub(crate) fn len(&self) -> usize {
        self.indices.len()
    }
}

impl Compositor {
    /// 输入、排列或终端大小有变化才需要重建画布。
    pub(crate) fn needs_frame(&self, plan: &FramePlan<'_>, area: Rect) -> bool {
        self.surface
            .as_ref()
            .is_none_or(|surface| surface.area() != area)
            || plan.changed_since(&self.placements)
    }

    /// 在保留画布上重建变化区域，再比较这些区域；调用者不交换或清空 Terminal 缓冲。
    /// 非 cell 寻址的图形控制序列使用完整画布，保持原有协议提交范围。
    pub(crate) fn render(
        &mut self,
        plan: FramePlan<'_>,
        frame: &mut Frame<'_>,
        whole_frame_graphics: bool,
    ) -> FrameUpdate {
        let resized = self
            .surface
            .as_ref()
            .is_none_or(|surface| surface.area() != frame.area());
        if resized {
            self.surface = None;
        }
        let surface = self
            .surface
            .get_or_insert_with(|| Surface::new(frame.area()));
        let placements = plan.placements();
        let (damage, painted_components) =
            plan.recompose(frame, &self.placements, resized || whole_frame_graphics);
        let indices = surface.diff(frame.buffer_mut(), &damage);
        self.placements = placements;
        FrameUpdate {
            painted_components,
            indices,
        }
    }

    /// 借出本次提交的数据，不拷贝整个画布或未变区域。
    pub(crate) fn updates<'a>(
        &'a self,
        update: &'a FrameUpdate,
    ) -> impl Iterator<Item = (u16, u16, &'a Cell)> + 'a {
        self.surface
            .iter()
            .flat_map(move |surface| surface.updates(&update.indices))
    }
}
