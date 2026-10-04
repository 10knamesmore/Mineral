//! 搜索组件按已确定的结果与详情布局准备显示状态和资源。

use ratatui::layout::Rect;

use crate::components::frame::PrepareCx;
use crate::components::lifecycle::Prepare;
use crate::runtime::state::SearchPage;

/// 搜索页结果区之外的布局选项；左侧结果区由准备入口的区域参数给出。
pub(crate) struct SearchPreparation {
    /// 本次布局是否显示右侧详情。
    pub(crate) detail_area: Option<Rect>,

    /// 页面转场是否接管主封面。
    pub(crate) cover_in_flight: bool,
}

impl Prepare for SearchPage {
    type Input<'a> = SearchPreparation;

    fn prepare(&mut self, area: Rect, input: SearchPreparation, cx: &mut PrepareCx<'_>) {
        super::panel::prepare_results(area, self, cx);
        if let Some(area) = input.detail_area {
            super::detail::prepare(area, self, cx, input.cover_in_flight);
        }
    }
}
