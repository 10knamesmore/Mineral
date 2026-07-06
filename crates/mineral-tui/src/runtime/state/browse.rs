//! Browse 布局层的 view 状态:列表导航 + 视图切换 + 全屏子模式 + 歌词显示 + `/` 过滤。
//!
//! 与模型数据(library / caps / player)分离——模型留在外层聚合态,Browse 决策时按需借入。
//! 全屏与 `/` 过滤都是这一页的内部子模式(同一套导航面),不另起独立页。

use mineral_config::AnimationConfig;
use mineral_model::PlaylistId;

use crate::render::anim::{Toggle, ticks16_from_ms};
use crate::runtime::deep_search::{self, DeepHit};
use crate::runtime::view_model::{PlaylistView, SongView};

use super::View;
use super::library::LibraryData;
use super::lyric::view::LyricView;
use super::nav::NavState;
use super::search::SearchState;
use super::view_switch::ViewSwitch;

/// Browse 视图逻辑所需的只读模型借用:歌曲库 + 配置——过滤 / 深度搜索 / 选中都读它,
/// 但这些是 model 数据(留在外层聚合态),故借入而非拥有。
#[derive(Clone, Copy)]
pub(crate) struct BrowseModel<'a> {
    /// 歌单 / 曲目库(过滤、选中、深度搜索的数据源)。
    pub library: &'a LibraryData,

    /// 全局配置(深度搜索权重 / 开关)。
    pub cfg: &'a mineral_config::Config,
}

/// Browse 布局层的 view 状态(光标 / 视图 / 全屏 / 歌词 / 过滤)。
pub struct BrowsePage {
    /// 左栏视图切换:Playlists ↔ Library 两态 + 横向过渡,由 [`ViewSwitch`] 合一。
    /// `current()` 给路由 / 选中语义、`eased_in_out()` 给渲染;`== View::X` 直接可比。
    pub view: ViewSwitch,

    /// 全屏播放态:逻辑开关(供按键路由)+ 进退场形变进度(供渲染),由 [`Toggle`] 合一。
    /// `on()` = 全屏、`eased_in_out()` = 形变位置。
    pub fullscreen: Toggle,

    /// 歌词面板显示态(副歌词档 + 全屏手动滚动脱离态)。
    pub lyric_view: LyricView,

    /// 列表浏览态(两个列表的光标 + 视口滚动、跨歌单位置记忆、选中变化时刻)。
    pub nav: NavState,

    /// `/` 模糊搜索状态(查询串 / 输入态 + 模糊匹配基建)。
    pub search: SearchState,
}

impl BrowsePage {
    /// 构造空 Browse 页(列表 / 过滤初始为空);视图切换与全屏形变拍数由动画配置折算。
    ///
    /// # Params:
    ///   - `anim`: 动画配置(取 view sweep / fullscreen 形变拍数)
    pub fn new(anim: &AnimationConfig) -> Self {
        let tick_ms = *anim.frame_tick_ms();
        Self {
            view: ViewSwitch::new(ticks16_from_ms(*anim.sweep_ms(), tick_ms)),
            fullscreen: Toggle::new(ticks16_from_ms(*anim.fullscreen_ms(), tick_ms)),
            lyric_view: LyricView::new(),
            nav: NavState::new(),
            search: SearchState::new(),
        }
    }
}

/// 视图逻辑:把过滤词 / 选中(本页状态)作用到歌曲库(借入的 model),算出当前可见 / 选中视图。
/// 外层聚合态留同名 forwarder 供渲染直读;本页执行器(按键路径)直接调这些。
impl BrowsePage {
    /// 当前选中歌单。
    /// - Playlists 视图:filtered 列表的索引(过滤词作用于 playlist 名)。
    /// - Library 视图:raw 列表的索引(进 Library 时已锁定为「用户进的那条」)。
    pub fn selected_playlist<'a>(&self, model: BrowseModel<'a>) -> Option<&'a PlaylistView> {
        match self.view.current() {
            View::Playlists => self
                .filtered_playlists(model)
                .get(self.nav.playlist.sel())
                .copied(),
            View::Library => model.library.playlists.get(self.nav.playlist.sel()),
        }
    }

    /// 当前选中歌单的曲目槽位(`None` = 还没拉到)。
    pub fn current_tracks_slot<'a>(&self, model: BrowseModel<'a>) -> Option<&'a Vec<SongView>> {
        self.selected_playlist(model)
            .and_then(|p| model.library.tracks.get(&p.data.id))
    }

    /// 当前选中歌单的曲目列表(slot 未到位时返回空)。
    pub fn current_tracks(&self, model: BrowseModel<'_>) -> Vec<SongView> {
        self.current_tracks_slot(model).cloned().unwrap_or_default()
    }

    /// 当前可见(被 search 过滤)的歌单列表。
    ///
    /// 空 query → 原序;非空 query → fzf 风格模糊匹配(拼音/首字母也算命中),
    /// 按 score 降序排,**stable** 保证同分按原序。
    pub fn filtered_playlists<'a>(&self, model: BrowseModel<'a>) -> Vec<&'a PlaylistView> {
        if self.search.query().is_empty() {
            return model.library.playlists.iter().collect();
        }
        self.search.sync_query();
        deep_search::ensure(&self.search, model.library, model.cfg);
        let deep = self.search.deep_cache.borrow();
        let mut scored: Vec<(f64, &PlaylistView)> = model
            .library
            .playlists
            .iter()
            .filter_map(|p| {
                let name = self
                    .search
                    .match_for(&p.data.name)
                    .map(|m| f64::from(m.score));
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
    /// 调用前提:本帧已有人调过 [`Self::filtered_playlists`],缓存已就绪;这里不再 ensure。
    pub fn deep_hit_for(&self, id: &PlaylistId) -> Option<DeepHit> {
        if self.search.query().is_empty() {
            return None;
        }
        self.search.deep_cache.borrow().hit_of(id).cloned()
    }

    /// 当前过滤结果里是否存在任何深度命中。调用前提同 [`Self::deep_hit_for`]。
    pub fn has_deep_hits(&self) -> bool {
        !self.search.query().is_empty() && self.search.deep_cache.borrow().has_hits()
    }

    /// 当前可见(被 search 过滤)的曲目列表。命中规则:歌名 / 别名 / 任一艺人 / 专辑名取最高分。
    pub fn filtered_tracks(&self, model: BrowseModel<'_>) -> Vec<SongView> {
        let tracks = self.current_tracks(model);
        if self.search.query().is_empty() {
            return tracks;
        }
        self.search.sync_query();
        let mut scored: Vec<(u32, SongView)> = tracks
            .into_iter()
            .filter_map(|sv| {
                let name = self.search.match_for(&sv.data.name).map(|m| m.score);
                // alias(译名/副标题)独立一段匹配,不与歌名拼接——否则「歌名 别名」被当整串,
                // 搜别名会因中间隔着歌名而错配。展示了 alias 就得能搜到它(否则搜它反被滤掉)。
                let alias = sv
                    .data
                    .alias
                    .as_deref()
                    .and_then(|a| self.search.match_for(a).map(|m| m.score));
                let artist = sv
                    .data
                    .artists
                    .iter()
                    .filter_map(|a| self.search.match_for(&a.name).map(|m| m.score))
                    .max();
                let album = sv
                    .data
                    .album
                    .as_ref()
                    .and_then(|a| self.search.match_for(&a.name).map(|m| m.score));
                let best = name
                    .into_iter()
                    .chain(alias)
                    .chain(artist)
                    .chain(album)
                    .max()?;
                Some((best, sv))
            })
            .collect();
        scored.sort_by_key(|&(s, _)| std::cmp::Reverse(s));
        scored.into_iter().map(|(_, sv)| sv).collect()
    }
}
