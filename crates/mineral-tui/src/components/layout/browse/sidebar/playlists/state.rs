//! 本列表实例的交互、搜索与显示状态。

use crate::runtime::deep_search::{self, DeepHit};
use crate::runtime::scroll::list::ScrollList;
use crate::runtime::state::LibraryData;
use crate::runtime::state::{ListExpansionState, SearchState};
use crate::runtime::view_model::PlaylistView;
use mineral_model::PlaylistId;

/// 随页面保留的歌单列表；共享数据由调用方借入。
pub struct PlaylistList {
    /// 本列表的光标、视口与位置标记。
    pub(crate) scroll: ScrollList,

    /// 本列表的查询与输入编辑状态。
    pub(crate) search: SearchState,

    /// 仅保留本列表清除筛选前的显示输入。
    pub(crate) expansion: ListExpansionState<PlaylistId>,

    /// 瞬态几何下不合成筛选展开。
    pub(super) stable: bool,
}

impl PlaylistList {
    /// 构造空列表，等待父页面借入数据。
    pub(crate) fn new() -> Self {
        Self {
            scroll: ScrollList::new(),
            search: SearchState::new(),
            expansion: ListExpansionState::default(),
            stable: false,
        }
    }
}

impl PlaylistList {
    /// 当前可见(被 search 过滤)的歌单列表。
    ///
    /// 空 query → 原序;非空 query → fzf 风格模糊匹配(拼音/首字母也算命中),
    /// 按 score 降序排,**stable** 保证同分按原序。
    pub(crate) fn rows<'a>(
        &self,
        library: &'a LibraryData,
        config: &mineral_config::Config,
    ) -> Vec<&'a PlaylistView> {
        let search = &self.search;
        if search.query().is_empty() {
            return library.playlists.iter().collect();
        }
        search.sync_query();
        deep_search::ensure(search, library, config);
        let deep = search.deep_cache.borrow();
        let mut scored: Vec<(f64, &PlaylistView)> = library
            .playlists
            .iter()
            .filter_map(|p| {
                let name = search.match_for(&p.data.name).map(|m| f64::from(m.score));
                let inner = deep.score_of(&p.data.id);
                let best = match (name, inner) {
                    (Some(n), Some(i)) => n.max(i),
                    (Some(n), None) => n,
                    (None, Some(i)) => i,
                    (None, None) => return None,
                };
                Some((best, p))
            })
            .collect();
        // total_cmp 全序 + sort_by 稳定:同分项保持原序。
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.into_iter().map(|(_, p)| p).collect()
    }

    /// 某歌单的深度命中展示载荷(克隆一份给渲染)。空 query / 无命中返回 `None`。
    ///
    /// 调用前提:本帧已调用 [`Self::rows`]，深度搜索缓存已经就绪。
    pub(crate) fn deep_hit_for(&self, id: &PlaylistId) -> Option<DeepHit> {
        if self.search.query().is_empty() {
            return None;
        }
        self.search.deep_cache.borrow().hit_of(id).cloned()
    }

    /// 当前过滤结果里是否存在任何深度命中。调用前提同 [`Self::deep_hit_for`]。
    pub(crate) fn has_deep_hits(&self) -> bool {
        !self.search.query().is_empty() && self.search.deep_cache.borrow().has_hits()
    }
}
