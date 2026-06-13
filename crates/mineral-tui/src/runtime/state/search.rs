//! 搜索状态域:本地模糊过滤(查询串 / 输入态 + matcher、预处理缓存、深度搜索缓存),
//! 外加远程搜索布局子域(Search 布局态开关 + per-channel 会话)。
//!
//! 只负责「单段文本怎么匹配」;跨集合的过滤排序(歌单 / 曲目列表)是
//! [`AppState`](crate::runtime::state::AppState) 的跨域查询,留在那边。

use std::cell::RefCell;
use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::render::anim::Toggle;
use crate::runtime::deep_search::DeepSearchCache;
use crate::runtime::filter::{FuzzyMatcher, Match, MatchableText};

/// 搜索状态([`AppState`](crate::runtime::state::AppState) 的搜索域)。
pub struct SearchState {
    /// 搜索关键字。
    pub query: String,

    /// 是否处于搜索输入态(`/` 触发,Enter / Esc 退出)。
    pub typing: bool,

    /// 本地搜索的模糊匹配器(fzf 风格子序列 + 中文拼音/首字母联合)。
    /// `&self` 路径下要复用 buffer,因此包 `RefCell`,与 `covers.protocols` 同理。
    pub matcher: RefCell<FuzzyMatcher>,

    /// 文本 → 预处理 [`MatchableText`] 的缓存。键是原始文本(歌名 / 艺人名 / 专辑名 /
    /// 歌单名),session 内长留;规模(每条 ~几百字节,总量上限 ≈ 已加载曲目数 × 3)
    /// 远低于其它 cache。换源 / 重启自然清掉。
    pub matchable_cache: RefCell<FxHashMap<String, Arc<MatchableText>>>,

    /// Playlists 深度搜索(搜索词穿透到歌单内歌曲)的结果缓存。按
    /// `(query, tracks 版本, 权重)` 失效,渲染帧只读;`RefCell` 与 matcher 同理。
    pub deep_cache: RefCell<DeepSearchCache>,

    /// 远程搜索布局子域(Search 布局态开关 + per-channel 会话)。
    pub remote_search: RemoteSearchState,
}

/// 远程搜索布局子域:Search 布局态开关 + per-channel 搜索会话。
///
/// `active` 与全屏态同级语义(全局布局态),但归在搜索域内(由 `s` 触发切换)。
pub struct RemoteSearchState {
    /// Search 布局态开关:`on()` 供按键路由互斥,`eased_in_out()` 供布局 morph 渲染位置。
    pub active: Toggle,
}

impl RemoteSearchState {
    /// 构造收起态的远程搜索域。`ticks` 为布局形变拍数(复用全屏节拍)。
    fn new(ticks: u16) -> Self {
        Self {
            active: Toggle::new(ticks),
        }
    }
}

impl SearchState {
    /// 构造空搜索态(无查询、非输入态、缓存全空)。
    ///
    /// # Params:
    ///   - `ticks`: Search 布局态形变拍数(复用全屏节拍)
    pub(crate) fn new(ticks: u16) -> Self {
        Self {
            query: String::new(),
            typing: false,
            matcher: RefCell::new(FuzzyMatcher::new()),
            matchable_cache: RefCell::new(FxHashMap::default()),
            deep_cache: RefCell::new(DeepSearchCache::default()),
            remote_search: RemoteSearchState::new(ticks),
        }
    }

    /// 把当前 `query` 同步给内部 matcher。空 query 也会被推下去,使 matcher 失活。
    /// 同 query 重复调用是无开销 no-op(matcher 内部判等)。
    pub fn sync_query(&self) {
        self.matcher.borrow_mut().set_query(&self.query);
    }

    /// 对单段文本跑一次匹配,返回 score + 已映射回原文 char 下标的 `hits`。
    ///
    /// 空 query / 不命中都返回 `None`。每帧渲染时按需调用(已带 MatchableText 缓存
    /// + matcher buffer 复用,开销可忽略)。
    pub fn match_for(&self, text: &str) -> Option<Match> {
        if self.query.is_empty() {
            return None;
        }
        self.sync_query();
        let mt = self.matchable_for(text);
        self.matcher.borrow_mut().score(&mt)
    }

    /// 拿 / 构造 一份预处理过的 `MatchableText`。首次见到的文本会算一次拼音。
    fn matchable_for(&self, text: &str) -> Arc<MatchableText> {
        if let Some(mt) = self.matchable_cache.borrow().get(text) {
            return Arc::clone(mt);
        }
        let mt = MatchableText::new(text);
        self.matchable_cache
            .borrow_mut()
            .insert(text.to_owned(), Arc::clone(&mt));
        mt
    }
}
