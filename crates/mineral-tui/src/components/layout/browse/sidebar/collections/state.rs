//! 歌单与专辑各自保留过滤词、光标和视口；类型抽屉只负责选择。

use rustc_hash::FxHashMap;

use super::drawer::TypeDrawer;
use crate::config::TuiConfig;
use crate::runtime::collection::{CollectionGroup, CollectionId, CollectionRef};
use crate::runtime::deep_search::{self, DeepHit};
use crate::runtime::scroll::list::ScrollList;
use crate::runtime::state::{LibraryData, ListExpansionState, SearchState};

/// 一种类型的列表状态，切换到另一种类型时继续保留。
pub(super) struct CollectionItems {
    /// 当前光标与视口。
    pub(super) scroll: ScrollList,

    /// 当前过滤与输入焦点。
    pub(super) search: SearchState,

    /// 按匹配分排列的集合身份；无查询时直接使用曲库顺序。
    search_results: Option<Vec<CollectionSearchResult>>,

    /// 清词时保留的旧显示输入。
    pub(super) expansion: ListExpansionState<CollectionId>,

    /// 稳态布局才合成筛选展开动画。
    pub(super) stable: bool,
}

/// 搜索命中的集合与得分，不依赖它在曲库列表中的位置。
struct CollectionSearchResult {
    collection: CollectionId,

    score: f64,
}

impl CollectionItems {
    /// 新列表从首项开始，无过滤条件。
    fn new() -> Self {
        Self {
            scroll: ScrollList::new(),
            search: SearchState::new(),
            search_results: None,
            expansion: ListExpansionState::default(),
            stable: false,
        }
    }
}

/// 曲库浏览入口；抽屉确认之前不改变当前列表类型。
pub struct CollectionList {
    /// 当前已经确认的列表类型。
    pub(super) group: CollectionGroup,

    /// 歌单的浏览位置和过滤词。
    pub(super) playlists: CollectionItems,

    /// 专辑的浏览位置和过滤词。
    pub(super) albums: CollectionItems,

    /// 类型选择焦点与展开动画。
    pub(super) drawer: TypeDrawer,
}

impl CollectionList {
    /// 从歌单开始浏览；抽屉使用配置中的横向切换时长。
    pub(crate) fn new(drawer_ticks: u16) -> Self {
        Self {
            group: CollectionGroup::Playlists,
            playlists: CollectionItems::new(),
            albums: CollectionItems::new(),
            drawer: TypeDrawer::new(drawer_ticks),
        }
    }

    /// 当前展示的类型。
    pub(crate) fn group(&self) -> CollectionGroup {
        self.group
    }

    /// 按类型借用独立列表状态。
    pub(super) fn state_for(&self, group: CollectionGroup) -> &CollectionItems {
        match group {
            CollectionGroup::Playlists => &self.playlists,
            CollectionGroup::Albums => &self.albums,
        }
    }

    /// 按类型修改列表状态。
    pub(super) fn state_for_mut(&mut self, group: CollectionGroup) -> &mut CollectionItems {
        match group {
            CollectionGroup::Playlists => &mut self.playlists,
            CollectionGroup::Albums => &mut self.albums,
        }
    }

    /// 当前列表的显示和过滤输入。
    pub(super) fn active(&self) -> &CollectionItems {
        self.state_for(self.group)
    }

    /// 当前列表的可变输入。
    pub(super) fn active_mut(&mut self) -> &mut CollectionItems {
        self.state_for_mut(self.group)
    }

    /// 返回另一层时按实体类型恢复父列表，不丢弃离开前的过滤条件。
    pub(crate) fn select_group(&mut self, group: CollectionGroup) {
        self.group = group;
    }

    /// 输入、配置或一批曲库数据变化后刷新；读取列表不会运行匹配器。
    pub(crate) fn refresh(&mut self, library: &LibraryData, config: &TuiConfig) {
        for group in [CollectionGroup::Playlists, CollectionGroup::Albums] {
            let state = self.state_for_mut(group);
            deep_search::refresh(&state.search, library, config, group);
            if state.search.query().is_empty() {
                state.search_results = None;
                continue;
            }
            let deep = state.search.deep_cache.borrow();
            let mut results: Vec<_> = collection_items(library, group)
                .into_iter()
                .filter_map(|item| {
                    let own = state.search.score_for(item.name()).map(f64::from);
                    let artist = item
                        .artists()
                        .iter()
                        .filter_map(|artist| state.search.score_for(&artist.name))
                        .max()
                        .map(f64::from);
                    let score = own
                        .into_iter()
                        .chain(artist)
                        .chain(deep.score_of(item.id()))
                        .max_by(f64::total_cmp)?;
                    Some(CollectionSearchResult {
                        collection: item.id(),
                        score,
                    })
                })
                .collect();
            results.sort_by(|a, b| b.score.total_cmp(&a.score));
            state.search_results = Some(results);
        }
    }

    /// 批量解析搜索命中的集合身份，只借出曲库中仍存在的项目。
    /// 临时身份表使解析为 O(曲库项目数 + 命中数)，不逐项扫描曲库。
    pub(crate) fn items<'a>(
        &self,
        library: &'a LibraryData,
        group: CollectionGroup,
    ) -> Vec<CollectionRef<'a>> {
        let Some(results) = &self.state_for(group).search_results else {
            return collection_items(library, group);
        };
        let by_id: FxHashMap<_, _> = match group {
            CollectionGroup::Playlists => library
                .playlists
                .iter()
                .map(|item| {
                    (
                        CollectionId::from(&item.data.id),
                        CollectionRef::Playlist(item),
                    )
                })
                .collect(),
            CollectionGroup::Albums => library
                .albums
                .iter()
                .map(|item| {
                    (
                        CollectionId::from(&item.data.id),
                        CollectionRef::Album(item),
                    )
                })
                .collect(),
        };
        results
            .iter()
            .filter_map(|result| by_id.get(&result.collection).copied())
            .collect()
    }

    /// 当前列表只含真实项目，列名固定在滚动区域之外。
    pub(crate) fn rows<'a>(&self, library: &'a LibraryData) -> Vec<CollectionRef<'a>> {
        self.items(library, self.group)
    }

    /// 某项目的最佳曲目命中，用于行内提示与进入后的定位。
    pub(crate) fn deep_hit_for(&self, id: impl Into<CollectionId>) -> Option<DeepHit> {
        if self.active().search.query().is_empty() {
            return None;
        }
        self.active().search.deep_cache.borrow().hit_of(id).cloned()
    }

    /// 当前查询是否需要显示命中曲目列。
    pub(crate) fn has_deep_hits(&self) -> bool {
        !self.active().search.query().is_empty()
            && self.active().search.deep_cache.borrow().has_hits()
    }
}

fn collection_items(library: &LibraryData, group: CollectionGroup) -> Vec<CollectionRef<'_>> {
    match group {
        CollectionGroup::Playlists => library
            .playlists
            .iter()
            .map(CollectionRef::Playlist)
            .collect(),
        CollectionGroup::Albums => library.albums.iter().map(CollectionRef::Album).collect(),
    }
}
