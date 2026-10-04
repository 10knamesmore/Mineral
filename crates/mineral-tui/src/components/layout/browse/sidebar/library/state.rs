//! 本列表实例的交互、搜索与显示状态。

use crate::runtime::scroll::list::ScrollList;
use crate::runtime::state::{ListExpansionState, SearchState};
use mineral_model::CollectionIndex;

/// 随页面保留的曲目列表；共享数据由调用方借入。
pub struct TrackList {
    /// 本列表的光标、视口与位置标记。
    pub(crate) scroll: ScrollList,

    /// 本列表选中标题的滚动状态。
    pub(crate) title: crate::runtime::marquee::Marquee,

    /// 本列表的查询与输入编辑状态。
    pub(crate) search: SearchState,

    /// 仅保留本列表清除筛选前的显示输入。
    pub(crate) expansion: ListExpansionState<CollectionIndex>,

    /// 本次列表是否处于稳定布局，瞬态合成不启动筛选展开。
    pub(super) stable: bool,

    /// 当前数据与查询的派生索引，读取只影响计算缓存。
    filtered: std::cell::RefCell<crate::runtime::state::TrackFilterCache>,
}

impl TrackList {
    /// 构造空列表，等待父页面借入数据。
    pub(crate) fn new() -> Self {
        Self {
            scroll: ScrollList::new(),
            title: crate::runtime::marquee::Marquee::default(),
            search: SearchState::new(),
            expansion: ListExpansionState::default(),
            stable: false,
            filtered: std::cell::RefCell::default(),
        }
    }

    /// 借用原始曲目；只缓存过滤顺序，不复制歌曲。
    pub(crate) fn filtered<'a>(
        &self,
        playlist: &mineral_model::PlaylistId,
        generation: u64,
        entries: &'a [crate::runtime::view_model::PlaylistEntryView],
    ) -> crate::runtime::state::FilteredTracks<'a> {
        self.filtered
            .borrow_mut()
            .view(playlist, generation, entries, &self.search)
    }
}
