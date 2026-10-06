//! 搜索页面借用的外部事实与本次绘制环境。

use crate::components::frame::FrameEnv;
use crate::runtime::state::SearchPage;
use mineral_model::SourceKind;
use rustc_hash::FxHashMap;

/// 搜索结果和提示行的只读视图。
pub(crate) struct SearchView<'a> {
    /// 搜索组件自己的会话与焦点。
    pub(crate) page: &'a SearchPage,

    /// 各来源支持的搜索能力。
    pub(crate) caps: &'a FxHashMap<SourceKind, mineral_channel_core::ChannelCaps>,

    /// 本次主题、配置和采样时间。
    pub(crate) frame: FrameEnv<'a>,

    /// 内联下拉框的布局环境。
    pub(crate) dock_right: bool,
}

impl SearchView<'_> {
    /// 搜索页沿用自身状态边界，声明当前结果与详情，不复制隐藏 source 的会话。
    pub(crate) fn dependencies(
        &self,
        areas: &crate::components::layout::shared::compute::Areas,
        detail: &super::detail::DetailView<'_>,
        inputs: &mut crate::render::memo::Dependencies<'_>,
    ) {
        self.page.dependencies(inputs);
        inputs.observe(self.caps);
        inputs.observe(&self.dock_right);
        inputs.observe(&self.page.active_results().is_some());
        if let Some(results) = self.page.active_results() {
            inputs.observe(&results.results);
            inputs.observe(&results.exhausted());
            results.list().dependencies(
                inputs,
                results.len(),
                usize::from(areas.left.height.saturating_sub(3)),
            );
            results
                .title
                .dependencies(inputs, self.frame.config.animation(), self.frame.now);
        }
        inputs.observe(detail.paint.liked);
        inputs.observe(&(detail.paint.focus, detail.paint.phase));
        inputs.observe(&detail.stack.is_some());
        if let (Some(stack), Some(area)) = (detail.stack, areas.right) {
            stack.dependencies(inputs, area, &detail.paint);
        }
    }
}
