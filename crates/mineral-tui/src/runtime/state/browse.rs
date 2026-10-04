//! Browse 布局层的 view 状态:列表导航 + 视图切换 + 全屏子模式 + 歌词显示 + `/` 过滤。
//!
//! 与模型数据(library / caps / player)分离——模型留在外层聚合态,Browse 决策时按需借入。
//! 全屏与 `/` 过滤都是这一页的内部子模式(同一套导航面),不另起独立页。

use mineral_config::{AnimationConfig, TrailTimingConfig};
use mineral_model::{PlaylistId, Song};

use crate::render::anim::{Toggle, TrailLeg, TrailingToggle, ticks16_from_ms};
use crate::runtime::deep_search::DeepHit;
use crate::runtime::view_model::{PlaylistEntryView, PlaylistView};

use super::View;
use super::library::LibraryData;
use super::nav::NavState;
use super::search::SearchState;
use super::track_filter::FilteredTracks;
use super::view_switch::ViewSwitch;
use crate::components::layout::browse::lyrics::LyricsPanel;
use crate::components::layout::browse::sidebar::{library::TrackList, playlists::PlaylistList};

/// Browse 视图逻辑所需的只读模型借用:歌曲库 + 配置——过滤 / 深度搜索 / 选中都读它,
/// 但这些是 model 数据(留在外层聚合态),故借入而非拥有。
#[derive(Clone, Copy)]
pub(crate) struct BrowseModel<'a> {
    /// 歌单 / 曲目库(过滤、选中、深度搜索的数据源)。
    pub library: &'a LibraryData,

    /// 全局配置(深度搜索权重 / 开关)。
    pub cfg: &'a mineral_config::Config,
}

/// Library 起播边界的 Song projection 与 exact target coordinate。
pub(crate) struct LibraryQueueProjection {
    /// 按配置范围投影出的播放队列。
    pub(crate) songs: Vec<Song>,

    /// `songs` 内选中 occurrence 的 0-based coordinate。
    pub(crate) target: usize,
}

/// Browse 布局层的 view 状态(光标 / 视图 / 全屏 / 歌词 / 过滤)。
pub struct BrowsePage {
    /// 当前选中项的详情面板。
    pub now_playing: crate::components::layout::browse::now_playing::NowPlaying,

    /// 左栏视图切换:Playlists ↔ Library 两态 + 横向过渡,由 [`ViewSwitch`] 合一。
    /// `current()` 给路由 / 选中语义、`eased_in_out()` 给渲染;`== View::X` 直接可比。
    pub view: ViewSwitch,

    /// 全屏播放态:逻辑开关(供按键路由)+ 进退场形变进度(供渲染),由 [`Toggle`] 合一。
    /// `on()` = 全屏、`eased_in_out()` = 形变位置。
    pub fullscreen: Toggle,

    /// 全屏氛围背景的滞后跟随:逻辑态跟随 `fullscreen`,但渲染进度慢半拍(follow-through)——
    /// 背景色落在几何形变后面淡入 / 淡出。`progress()` 给背景浓度、`active()` 决定是否铺场
    /// (退出时几何已回列表、背景仍越过列表慢褪,故需独立存活判定)。
    pub ambient_reveal: TrailingToggle,

    /// 歌词面板显示态(副歌词档 + 全屏手动滚动脱离态)。
    pub lyrics: LyricsPanel,

    /// 列表浏览态(两个列表的光标 + 视口滚动、跨歌单位置记忆、选中变化时刻)。
    pub nav: NavState,

    /// 歌单列表组件，返回时保留自己的查询和光标。
    pub playlists: PlaylistList,

    /// 已打开歌单的曲目列表组件。
    pub tracks: TrackList,
}

impl BrowsePage {
    /// 构造空 Browse 页(列表 / 过滤初始为空);视图切换与全屏形变拍数由动画配置折算。
    ///
    /// # Params:
    ///   - `anim`: 动画配置(取 view sweep / fullscreen 形变拍数)
    pub fn new(anim: &AnimationConfig) -> Self {
        let tick_ms = *anim.frame_tick_ms();
        let trail = anim.ambient_trail();
        Self {
            now_playing: Default::default(),
            view: ViewSwitch::new(ticks16_from_ms(*anim.sweep_ms(), tick_ms)),
            fullscreen: Toggle::new(ticks16_from_ms(*anim.fullscreen_ms(), tick_ms)),
            ambient_reveal: TrailingToggle::new(
                trail_leg(trail.enter(), tick_ms),
                trail_leg(trail.exit(), tick_ms),
            ),
            lyrics: LyricsPanel::new(),
            nav: NavState::new(),
            playlists: PlaylistList::new(),
            tracks: TrackList::new(),
        }
    }

    /// 数据或尺寸改变后，两份旧显示输入均失效。
    pub(crate) fn invalidate_expansions(&mut self) {
        self.playlists.invalidate();
        self.tracks.invalidate();
    }

    /// 输入立即接管正在展开的列表。
    pub(crate) fn interrupt_expansions(&mut self) {
        self.playlists.interrupt();
        self.tracks.interrupt();
    }

    /// 当前接受按键的列表查询；离屏面板应直接读取自己的搜索状态。
    pub fn active_search(&self) -> &SearchState {
        match self.view.current() {
            View::Playlists => self.playlists.search(),
            View::Library => self.tracks.search(),
        }
    }

    /// 测试直接装配保留的查询；生产输入交给具体列表。
    #[cfg(test)]
    pub(crate) fn active_search_mut(&mut self) -> &mut SearchState {
        match self.view.current() {
            View::Playlists => self.playlists.test_search_mut(),
            View::Library => self.tracks.test_search_mut(),
        }
    }

    /// 当前打开歌单对应的窄列表输入。
    pub(crate) fn track_input<'a>(
        &self,
        model: BrowseModel<'a>,
    ) -> crate::components::layout::browse::sidebar::library::TrackInput<'a> {
        let playlist = self.opened_playlist(model);
        crate::components::layout::browse::sidebar::library::TrackInput {
            playlist,
            tracks: playlist.and_then(|playlist| model.library.tracks.get(&playlist.data.id)),
            generation: model.library.tracks_generation,
            playing: None,
        }
    }

    /// 推进氛围背景滞后跟随一拍:先跟随全屏逻辑态(变化才重置滞后),再推进(follow-through)。
    /// 与 `fullscreen.tick()` 同拍调用。
    pub fn tick_ambient_reveal(&mut self) {
        self.ambient_reveal.follow(self.fullscreen.on());
        self.ambient_reveal.tick();
    }

    /// 重折动画拍数(配置热更):保留视图 / 全屏的逻辑态与动画相位,只换速度。
    ///
    /// # Params:
    ///   - `anim`: 新动画配置
    pub fn retempo(&mut self, anim: &AnimationConfig) {
        self.lyrics.extra_press.retempo(anim);
        let tick_ms = *anim.frame_tick_ms();
        let trail = anim.ambient_trail();
        self.playlists
            .retempo(ticks16_from_ms(*anim.list_scroll_ms(), tick_ms));
        self.tracks
            .retempo(ticks16_from_ms(*anim.list_scroll_ms(), tick_ms));
        self.view
            .retempo(ticks16_from_ms(*anim.sweep_ms(), tick_ms));
        self.fullscreen
            .retempo(ticks16_from_ms(*anim.fullscreen_ms(), tick_ms));
        self.ambient_reveal.retempo(
            trail_leg(trail.enter(), tick_ms),
            trail_leg(trail.exit(), tick_ms),
        );
    }
}

/// 把配置里一个方向的滞后跟随时长折算成运行时 [`TrailLeg`](毫秒 → 拍数)。
fn trail_leg(timing: &TrailTimingConfig, tick_ms: u64) -> TrailLeg {
    TrailLeg::new(
        ticks16_from_ms(*timing.delay_ms(), tick_ms),
        ticks16_from_ms(*timing.ease_ms(), tick_ms),
    )
}

/// 视图逻辑:把过滤词 / 选中(本页状态)作用到歌曲库(借入的 model),算出当前可见 / 选中视图。
/// 外层聚合态留同名 forwarder 供渲染直读;本页执行器(按键路径)直接调这些。
impl BrowsePage {
    /// 当前选中歌单：外层取搜索结果中的光标，歌单内按进入时保存的身份取值。
    pub fn selected_playlist<'a>(&self, model: BrowseModel<'a>) -> Option<&'a PlaylistView> {
        match self.view.current() {
            View::Playlists => self.selected_playlist_in_list(model),
            View::Library => self.opened_playlist(model),
        }
    }

    /// 外层列表光标指向的歌单；过渡期间绘制 Playlists 端时不跟随目标视图。
    pub fn selected_playlist_in_list<'a>(
        &self,
        model: BrowseModel<'a>,
    ) -> Option<&'a PlaylistView> {
        self.filtered_playlists(model)
            .get(self.playlists.scroll().sel())
            .copied()
    }

    /// 已打开歌单；返回动画期间仍保留，避免离场的曲目面板跟随外层光标变化。
    pub fn opened_playlist<'a>(&self, model: BrowseModel<'a>) -> Option<&'a PlaylistView> {
        let id = self.nav.opened_playlist.as_ref()?;
        model.library.playlists.iter().find(|p| &p.data.id == id)
    }

    /// 已打开歌单的曲目槽位(`None` = 尚未进入歌单或还没拉到)。
    pub fn current_tracks_slot<'a>(
        &self,
        model: BrowseModel<'a>,
    ) -> Option<&'a Vec<PlaylistEntryView>> {
        self.opened_playlist(model)
            .and_then(|p| model.library.tracks.get(&p.data.id))
            .map(|tracks| &tracks.entries)
    }

    /// 已打开歌单的曲目列表(slot 未到位时返回空)。
    pub fn current_tracks<'a>(&self, model: BrowseModel<'a>) -> &'a [PlaylistEntryView] {
        self.current_tracks_slot(model).map_or(&[], Vec::as_slice)
    }

    /// 借用歌单列表组件维护的过滤结果。
    pub fn filtered_playlists<'a>(&self, model: BrowseModel<'a>) -> Vec<&'a PlaylistView> {
        self.playlists.rows(model.library, model.cfg)
    }

    /// 借用该歌单的深度命中展示内容。
    pub fn deep_hit_for(&self, id: &PlaylistId) -> Option<DeepHit> {
        self.playlists.deep_hit_for(id)
    }

    /// 当前可见(被 search 过滤)的曲目列表。命中规则:歌名 / 别名 / 任一艺人 / 专辑名取最高分。
    pub(crate) fn filtered_tracks<'a>(&self, model: BrowseModel<'a>) -> FilteredTracks<'a> {
        let Some(playlist) = self.opened_playlist(model) else {
            return FilteredTracks::empty();
        };
        let Some(entries) = model.library.tracks.get(&playlist.data.id) else {
            return FilteredTracks::empty();
        };
        self.tracks
            .filtered(&playlist.data.id, model.library.tracks_generation, entries)
    }

    /// 按 `behavior.filter_play_scope` 建立 Library 起播队列，并保留过滤结果中选中的 exact
    /// collection occurrence。
    ///
    /// `Collection` 档用 [`mineral_model::CollectionIndex`] 映射回完整 collection，不能按
    /// SongId first-match，因为 Playlist 允许同一 Song 出现多次；`Matches` 档直接使用当前
    /// filtered projection。选择失效时返回 `None`。
    ///
    /// # Params:
    ///   - `model`: 当前 library 与配置借用
    ///
    /// # Return:
    ///   可原子提交的 queue projection；当前选择无对应曲目时为 `None`。
    pub(crate) fn library_queue_projection(
        &self,
        model: BrowseModel<'_>,
    ) -> Option<LibraryQueueProjection> {
        let filtered = self.filtered_tracks(model);
        let filtered_target = self.tracks.scroll().sel();
        let selected_index = filtered.get(filtered_target)?.data.index;
        if model
            .cfg
            .tui()
            .behavior()
            .filter_play_scope()
            .matches_only()
        {
            Some(LibraryQueueProjection {
                songs: filtered
                    .iter()
                    .map(|entry| entry.data.song.clone())
                    .collect(),
                target: filtered_target,
            })
        } else {
            let entries = self.current_tracks(model);
            let target = entries
                .iter()
                .position(|entry| entry.data.index == selected_index)?;
            Some(LibraryQueueProjection {
                songs: entries
                    .iter()
                    .map(|entry| entry.data.song.clone())
                    .collect(),
                target,
            })
        }
    }
}
