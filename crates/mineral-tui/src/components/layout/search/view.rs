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
