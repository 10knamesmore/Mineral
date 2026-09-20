//! Browse 页的按键行为:列表光标移动 / 视口滚动 / 视图进入与返回 / `/` 过滤输入。
//!
//! 执行器长在 `impl BrowsePage` 上(Page 自管 view 态),经 [`Page::on_key`] 按子模式分派;
//! 需要 model(client 起播 / 落盘 / lyrics)的副作用作为 [`BrowseEffect`] 冒泡,
//! [`App::apply_browse_effect`] 落地。App 侧留同名 forwarder 供 `dispatch` 表(Search 面板等
//! 回落)直调。全屏 / `/` 过滤是 Browse 的子模式,屏蔽闸 / 分流在各执行器开头判。

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mineral_model::{PlaylistId, Song};
use mineral_task::{ChannelFetchKind, Priority, TaskKind};

use crate::render::anim::ticks16_from_ms;
use crate::runtime::action::{Action, ScrollStep, SelectionMove};
use crate::runtime::keymap::{Keymap, chord_from_event};
use crate::runtime::line_input::InputRequest;
use crate::runtime::scroll;
use crate::runtime::state::{BrowseModel, BrowsePage, View};
use crate::runtime::track_pos::{PendingRestore, TrackPos};

use crate::app::App;
use crate::app::page::Page;

/// Browse 页吃完按键后吐给 App 的副作用意图;[`App::apply_browse_effect`] 落地。
/// Browse 自管纯 view 态动作;需要 model(client / 落盘 / lyrics)的操作经此冒泡。
pub(crate) enum BrowseEffect {
    /// 原子替换队列并起播 exact target occurrence。
    PlayQueue {
        /// 替换进播放队列的曲目；Library 过滤态的范围由 `filter_play_scope` 决定。
        queue: Vec<Song>,

        /// `queue` 内的 0-based exact target coordinate。
        target: usize,

        /// 队列语境(埋点 provenance:库内起播恒 Playlist{当前歌单 id})。
        context: mineral_protocol::QueueContextWire,
    },

    /// 全屏态滚动歌词(歌词数据是 model,落地走既有 [`AppState::scroll_lyrics`])。
    ScrollLyrics(ScrollStep),

    /// 全屏手动浏览(已脱离)时 Enter 跳到焦点行时间点(seek + 回附着态);
    /// 落地走 [`AppState::lyric_focus_seek_target`],无时间戳行则无跳转目标。
    SeekLyricFocus,

    /// 把当前 track_pos 记忆表落盘(persist 档;表本身已在 Browse 内更新)。
    PersistTrackPos,

    /// 进搜索态时补拉这些歌单的曲目(Background 优先级);落地时提任务 + 标记已请求。
    SubmitDeepSearch(Vec<PlaylistId>),

    /// 非 Browse 自管的动词回落全局 dispatch(transport / 菜单 / 全屏切换等)。
    Dispatch(Action),

    /// 纯状态改动,无副作用。
    None,
}

/// Browse 页决策所需的只读跨页上下文(props):由 App 在调用点就地构造。
#[derive(Clone, Copy)]
pub(crate) struct BrowseCtx<'a> {
    /// 只读模型借用(library + cfg):过滤 / 选中 / 深度搜索读它。
    model: BrowseModel<'a>,

    /// 键位表(把按键解析成 [`Action`])。
    keymap: &'a Keymap,
}

impl Page for BrowsePage {
    type Effect = BrowseEffect;
    type Ctx<'a> = BrowseCtx<'a>;

    /// Browse 布局态按键入口:`/` 过滤 typing 子模式优先吞键;否则按 keymap 命中分派给自管动词
    /// (移动 / 滚动 / 进退 / 进搜索),非 Browse 动词(transport / 菜单 / 全屏切换等)回落 Dispatch。
    fn on_key(&mut self, key: &KeyEvent, ctx: BrowseCtx<'_>) -> BrowseEffect {
        // `/` 模糊过滤输入子模式:吞键进过滤词,优先于动作解析。
        if self.active_search().typing {
            self.handle_search_key(key, ctx.model);
            return BrowseEffect::None;
        }
        match chord_from_event(key).and_then(|c| ctx.keymap.lookup(c)) {
            Some(Action::MoveSelection(mv)) => {
                self.move_selection(mv, ctx.model);
                BrowseEffect::None
            }
            Some(Action::Scroll(step)) => self.scroll(step, ctx.model),
            Some(Action::EnterSearch) => self.enter_search(ctx.model),
            Some(Action::ActivateSelection) => self.activate_selection(ctx.model),
            Some(Action::BackOrClearSearch) => self.back_or_clear_search(ctx.model),
            Some(action) => BrowseEffect::Dispatch(action),
            None => BrowseEffect::None,
        }
    }
}

/// Browse 页的按键执行器(纯 view 态改动在此;需要 model 的副作用经 [`BrowseEffect`] 冒泡)。
impl BrowsePage {
    /// 列表光标移动,按当前 view 落到 `nav.sel_playlist` / `nav.sel_track`,越界钳首末行;
    /// 全屏态屏蔽(屏上无列表)。
    fn move_selection(&mut self, mv: SelectionMove, model: BrowseModel<'_>) {
        if self.fullscreen.on() {
            return;
        }
        self.nav.last_sel_change = Instant::now();
        // len 先算(释放对 model 的不可变借用),再取列表态可变借用。
        let len = match self.view.current() {
            View::Playlists => self.filtered_playlists(model).len(),
            View::Library => self.filtered_tracks(model).len(),
        };
        let list = match self.view.current() {
            View::Playlists => &mut self.nav.playlist,
            View::Library => &mut self.nav.track,
        };
        list.move_by(mv, len);
    }

    /// `<C-d>` 族滚动按上下文路由:全屏吐 [`BrowseEffect::ScrollLyrics`] 滚歌词;浏览态滚当前列表
    /// ——视口目标与光标同移 n 行(vim `<C-d>` 语义,保持光标屏上相对位置),边界由渲染端统一钳。
    fn scroll(&mut self, step: ScrollStep, model: BrowseModel<'_>) -> BrowseEffect {
        if self.fullscreen.on() {
            return BrowseEffect::ScrollLyrics(step);
        }
        let delta = scroll::viewport::step_delta(step, model.cfg.tui().behavior());
        let anim = model.cfg.tui().animation();
        let ticks = ticks16_from_ms(*anim.list_scroll_ms(), *anim.frame_tick_ms());
        // len 先算(释放 model 借用),再取列表态可变借用;page 同移视口 + 光标(vim `<C-d>` 语义)。
        let len = match self.view.current() {
            View::Playlists => self.filtered_playlists(model).len(),
            View::Library => self.filtered_tracks(model).len(),
        };
        self.nav.last_sel_change = Instant::now();
        let list = match self.view.current() {
            View::Playlists => &mut self.nav.playlist,
            View::Library => &mut self.nav.track,
        };
        list.page(delta, len, ticks);
        BrowseEffect::None
    }

    /// 进入搜索输入态并清词;全屏态屏蔽(屏上无列表可滤)。同时吐出待补拉歌单(深度搜索数据保障)。
    fn enter_search(&mut self, model: BrowseModel<'_>) -> BrowseEffect {
        if self.fullscreen.on() {
            return BrowseEffect::None;
        }
        self.clear_search(model);
        self.active_search_mut().typing = true;
        let pending = self.deep_search_pending(model);
        if pending.is_empty() {
            BrowseEffect::None
        } else {
            BrowseEffect::SubmitDeepSearch(pending)
        }
    }

    /// 深度搜索数据保障:Playlists 视图 + deep 开启时,列出所有未拉取 / 未请求的歌单(待补拉);
    /// 非该状态返回空；落地端交给统一提交层并去重，容量不足由提交层重试。
    fn deep_search_pending(&self, model: BrowseModel<'_>) -> Vec<PlaylistId> {
        if self.view != View::Playlists || !*model.cfg.tui().search().deep().enabled() {
            return Vec::new();
        }
        model
            .library
            .playlists
            .iter()
            .map(|p| &p.data.id)
            .filter(|id| {
                model
                    .library
                    .needs_playlist(id, mineral_channel_core::PlaylistLoad::Complete, true)
            })
            .cloned()
            .collect()
    }

    /// 搜索词变化后重置当前列表的光标和视口；清除筛选时另行保留实体位置。
    fn reset_sel_for_search(&mut self) {
        match self.view.current() {
            View::Playlists => self.nav.playlist.place(0, 0),
            View::Library => self.nav.track.place(0, 0),
        }
    }

    /// 搜索输入态按键:Esc 清除并退出，Enter 保留查询并退出，空词上 Backspace 退出。
    /// 改词后选中首个结果；删掉最后一个字符时按清除筛选处理，保留选中实体。
    ///
    /// 带 CONTROL 的字符键一律吞掉不当输入——否则 `<C-d>` 族滚动键在搜索态会把裸字符塞进 query。
    fn handle_search_key(&mut self, key: &KeyEvent, model: BrowseModel<'_>) {
        match key.code {
            KeyCode::Esc => {
                self.active_search_mut().typing = false;
                self.clear_search(model);
            }
            KeyCode::Enter => {
                self.active_search_mut().typing = false;
            }
            KeyCode::Backspace => {
                if self.active_search().query().is_empty() {
                    self.active_search_mut().typing = false;
                    return;
                }
                if self.active_search().query().chars().count() == 1
                    && !self.active_search().query_split().0.is_empty()
                {
                    self.clear_search(model);
                    self.nav.last_sel_change = Instant::now();
                } else if self.active_search_mut().edit(InputRequest::DeletePrev) {
                    self.reset_sel_for_search();
                    self.nav.last_sel_change = Instant::now();
                }
            }
            KeyCode::Char(_) if key.modifiers.contains(KeyModifiers::CONTROL) => {}
            KeyCode::Char(c) => {
                self.active_search_mut().edit(InputRequest::Insert(c));
                self.reset_sel_for_search();
                self.nav.last_sel_change = Instant::now();
            }
            KeyCode::Left => {
                self.active_search_mut().edit(InputRequest::Left);
            }
            KeyCode::Right => {
                self.active_search_mut().edit(InputRequest::Right);
            }
            KeyCode::Home => {
                self.active_search_mut().edit(InputRequest::Home);
            }
            KeyCode::End => {
                self.active_search_mut().edit(InputRequest::End);
            }
            _ => {}
        }
    }

    /// 在当前视图「进入」:Playlists 进选中歌单的 Library(含深度命中定位 / 位置记忆恢复);
    /// Library 吐 [`BrowseEffect::PlayQueue`] 起播选中曲。全屏态改吐
    /// [`BrowseEffect::SeekLyricFocus`](脱离浏览时跳到焦点行)/ 附着态无焦点则吞掉。
    fn activate_selection(&mut self, model: BrowseModel<'_>) -> BrowseEffect {
        if self.fullscreen.on() {
            // 全屏手动浏览(已脱离,有焦点行)→ Enter 跳到焦点行时间点;附着态无焦点,吞掉。
            return if self.lyric_view.scroll.is_some() {
                BrowseEffect::SeekLyricFocus
            } else {
                BrowseEffect::None
            };
        }
        self.nav.last_sel_change = Instant::now();
        match self.view.current() {
            View::Playlists => {
                // 上一次进歌单挂的延迟恢复(曲目未到就退出来了)就此作废。
                self.nav.pending_track_restore = None;
                let mut sel_track = 0usize;
                // 记忆恢复时的屏上相对行;None = 默认落位(光标上方留 scrolloff)。
                let mut screen_anchor: Option<usize> = None;
                let Some(target_id) = self
                    .filtered_playlists(model)
                    .get(self.nav.playlist.sel())
                    .map(|p| p.data.id.clone())
                else {
                    return BrowseEffect::None;
                };
                // 保留父列表查询与位置；曲目面板用独立身份和查询，过渡两侧不会互相改写。
                let locate = (*model.cfg.tui().search().deep().locate_on_enter())
                    .then(|| self.deep_hit_for(&target_id).map(|h| h.song_id))
                    .flatten();
                self.nav.opened_playlist = Some(target_id.clone());
                self.search.tracks.clear();
                self.search.tracks.typing = false;
                if let Some(idx) = locate.and_then(|song_id| {
                    model.library.tracks.get(&target_id).and_then(|entries| {
                        entries
                            .iter()
                            .position(|entry| entry.data.song.id == song_id)
                    })
                }) {
                    sel_track = idx;
                } else if model.cfg.tui().behavior().remember_track_pos().enabled()
                    && let Some(pos) = self.nav.track_pos.get(&target_id).cloned()
                {
                    // 深度命中优先于历史位置；尚未加载到记忆曲目时延迟恢复。
                    if let Some(tracks) = model.library.tracks.get(&target_id).filter(|tracks| {
                        tracks.complete
                            || tracks.iter().any(|entry| entry.data.song.id == pos.song_id)
                    }) {
                        sel_track = pos.resolve(tracks);
                        screen_anchor = Some(pos.screen_row);
                    } else {
                        self.nav.pending_track_restore = Some(PendingRestore {
                            playlist: target_id.clone(),
                            pos,
                        });
                    }
                }
                mineral_log::debug!(
                    target: "tui",
                    playlist = %target_id,
                    filtered = !self.search.playlists.query().is_empty(),
                    playlist_selection = self.nav.playlist.sel(),
                    playlist_scroll = self.nav.playlist.scroll_target(),
                    track_selection = sel_track,
                    "open playlist with retained list context"
                );
                self.view.switch_to(View::Library);
                // 光标落位 + 视口瞬时定位(记忆按屏上相对行还原;命中歌上方留 scrolloff;无命中即从头看)。
                let anchor = screen_anchor
                    .unwrap_or_else(|| usize::from(*model.cfg.tui().behavior().scrolloff()));
                self.nav.track.place(sel_track, anchor);
                BrowseEffect::None
            }
            View::Library => {
                let Some(projection) = self.library_queue_projection(model) else {
                    return BrowseEffect::None;
                };
                let context = self.opened_playlist(model).map_or(
                    mineral_protocol::QueueContextWire::Unknown,
                    |p| mineral_protocol::QueueContextWire::Playlist {
                        id: p.data.id.clone(),
                        name: Some(p.data.name.clone()),
                    },
                );
                // Server 端按 PlayMode 决定要不要洗牌;client 只发原始 queue + target。
                BrowseEffect::PlayQueue {
                    queue: projection.songs,
                    target: projection.target,
                    context,
                }
            }
        }
    }

    /// 清除当前层筛选，按实体身份映射回完整列表，并尽量保持光标的屏幕行。
    fn clear_search(&mut self, model: BrowseModel<'_>) {
        if self.active_search().query().is_empty() {
            return;
        }
        let (list, index) = match self.view.current() {
            View::Playlists => {
                let selected = self
                    .filtered_playlists(model)
                    .get(self.nav.playlist.sel())
                    .map(|p| p.data.id.clone());
                self.search.playlists.clear();
                let index = selected
                    .and_then(|id| model.library.playlists.iter().position(|p| p.data.id == id));
                (&mut self.nav.playlist, index)
            }
            View::Library => {
                let selected = self
                    .filtered_tracks(model)
                    .get(self.nav.track.sel())
                    .map(|entry| entry.data.index);
                self.search.tracks.clear();
                let index = selected.and_then(|index| {
                    self.current_tracks(model)
                        .iter()
                        .position(|entry| entry.data.index == index)
                });
                (&mut self.nav.track, index)
            }
        };
        let previous_selection = list.sel();
        let screen_row = previous_selection.saturating_sub(list.scroll_target());
        if let Some(index) = index {
            list.place(index, screen_row);
        } else {
            list.place(0, 0);
        }
        mineral_log::debug!(
            target: "tui",
            view = ?self.view.current(),
            previous_selection,
            selection = list.sel(),
            screen_row,
            "clear list filter preserving selected item"
        );
    }

    /// 返回时恢复父列表中的已打开歌单；异步刷新改变排序时按身份重新定位。
    fn restore_playlist_selection(&mut self, model: BrowseModel<'_>) {
        let index = self.nav.opened_playlist.as_ref().and_then(|id| {
            self.filtered_playlists(model)
                .iter()
                .position(|p| &p.data.id == id)
        });
        if let Some(index) = index
            && index != self.nav.playlist.sel()
        {
            let screen_row = self
                .nav
                .playlist
                .sel()
                .saturating_sub(self.nav.playlist.scroll_target());
            self.nav.playlist.place(index, screen_row);
        }
    }

    /// 当前层有搜索先清除并保留选中项，否则从歌单返回保留的父列表。全屏态屏蔽。
    fn back_or_clear_search(&mut self, model: BrowseModel<'_>) -> BrowseEffect {
        if self.fullscreen.on() {
            return BrowseEffect::None;
        }
        self.nav.last_sel_change = Instant::now();
        if !self.active_search().query().is_empty() {
            self.clear_search(model);
            return BrowseEffect::None;
        }
        if matches!(self.view.current(), View::Library) {
            let persist = self.remember_track_pos(model);
            self.nav.pending_track_restore = None;
            self.restore_playlist_selection(model);
            mineral_log::debug!(
                target: "tui",
                playlist = ?self.nav.opened_playlist,
                filtered = !self.search.playlists.query().is_empty(),
                selection = self.nav.playlist.sel(),
                scroll = self.nav.playlist.scroll_target(),
                "return to retained playlist list"
            );
            self.view.switch_to(View::Playlists);
            if persist {
                return BrowseEffect::PersistTrackPos;
            }
        }
        BrowseEffect::None
    }

    /// 把 Library 当前光标记入位置记忆表(`behavior.remember_track_pos` 非 off);更新内存表,
    /// 返回是否需要落盘(persist 档)——落盘由调用端做。
    ///
    /// 曲目未就绪 / 空歌单时不记,**保留旧记忆**——进了还没加载完的歌单就退出来,不该把上次的
    /// 有效位置抹成空。搜索过滤态下 `nav.sel_track` 指向 filtered 列表,记忆统一锚定到 raw 下标。
    fn remember_track_pos(&mut self, model: BrowseModel<'_>) -> bool {
        let mem = *model.cfg.tui().behavior().remember_track_pos();
        if !mem.enabled() || self.view != View::Library {
            return false;
        }
        let Some(pid) = self.selected_playlist(model).map(|p| p.data.id.clone()) else {
            return false;
        };
        let Some(song_id) = self
            .filtered_tracks(model)
            .get(self.nav.track.sel())
            .map(|entry| entry.data.song.id.clone())
        else {
            return false;
        };
        let index = self
            .current_tracks(model)
            .iter()
            .position(|entry| entry.data.song.id == song_id)
            .unwrap_or(self.nav.track.sel());
        // 屏上相对行:光标减当前滚动目标(渲染端维护的视口首行)。
        let screen_row = self
            .nav
            .track
            .sel()
            .saturating_sub(self.nav.track.scroll_target());
        self.nav.track_pos.insert(
            pid,
            TrackPos {
                song_id,
                index,
                screen_row,
            },
        );
        mem.persists()
    }
}

impl App {
    /// Browse 布局态按键入口:就地构造 [`BrowseCtx`]、交 Browse 页吃键、再落地它吐回的意图。
    pub(in crate::app) fn handle_browse_key(&mut self, key: &KeyEvent) {
        // ctx 就地构造:browse / library / cfg 是 self.state 三个不相交字段,keymap 在 self 上,
        // 全 disjoint,借用检查器放行。
        let ctx = BrowseCtx {
            model: BrowseModel {
                library: &self.state.library,
                cfg: &self.state.cfg,
            },
            keymap: &self.keymap,
        };
        let eff = self.state.browse.on_key(key, ctx);
        self.apply_browse_effect(eff);
    }

    /// 落地 Browse 页吐回的副作用意图。Browse 只产意图,client / 落盘 / lyrics 全在此收口。
    fn apply_browse_effect(&mut self, eff: BrowseEffect) {
        match eff {
            BrowseEffect::PlayQueue {
                queue,
                target,
                context,
            } => self.play_queue(queue, target, context),
            BrowseEffect::ScrollLyrics(step) => self.state.scroll_lyrics(step),
            BrowseEffect::SeekLyricFocus => {
                // 焦点行有时间戳才跳:seek 到该行起点,并把锚点钉在该行等 seek 落地再无缝
                // 回附着(不立即清脱离——seek 往返滞后会先跳回旧播放行)。无时间戳行无跳转
                // 目标,保持脱离态不动。
                if let (Some(ms), Some(line)) = (
                    self.state.lyric_focus_seek_target(),
                    self.state.manual_lyric_focus_line(),
                ) {
                    self.client.seek(ms);
                    self.state.hold_lyric_anchor_for_seek(line);
                }
            }
            BrowseEffect::PersistTrackPos => self
                .ui_prefs
                .save_track_positions(&self.state.browse.nav.track_pos),
            BrowseEffect::SubmitDeepSearch(ids) => {
                let selected = self
                    .state
                    .filtered_playlists()
                    .get(self.state.browse.nav.playlist.sel())
                    .map(|playlist| playlist.data.id.clone());
                for id in ids {
                    let priority = if selected.as_ref() == Some(&id) {
                        Priority::User
                    } else {
                        Priority::Background
                    };
                    self.client.submit_task(
                        TaskKind::ChannelFetch(ChannelFetchKind::PlaylistDetail {
                            id: id.clone(),
                            load: mineral_channel_core::PlaylistLoad::Complete,
                        }),
                        priority,
                    );
                    self.state
                        .library
                        .request_playlist(id, mineral_channel_core::PlaylistLoad::Complete);
                }
            }
            BrowseEffect::Dispatch(action) => self.dispatch(action),
            BrowseEffect::None => {}
        }
    }

    /// 进入搜索输入态 forwarder(dispatch 走它);逻辑在 [`BrowsePage::enter_search`]。
    pub(in crate::app) fn enter_search(&mut self) {
        let eff = self.state.browse.enter_search(BrowseModel {
            library: &self.state.library,
            cfg: &self.state.cfg,
        });
        self.apply_browse_effect(eff);
    }

    /// `c` 跳到 Library 里的在播曲:按歌曲身份在过滤视图里定位,同曲多份取首个
    /// ——曲目没有像 queue 那样的在播位置锚点。只移光标,视口交给渲染端 Advancing
    /// 缓动滚过去;歌单面 / 全屏态与无在播 / 无命中时空操作。
    pub(in crate::app) fn jump_to_current(&mut self) {
        if self.state.browse.fullscreen.on() || self.state.browse.view.current() != View::Library {
            return;
        }
        let Some(playing) = self.state.playback.track.as_ref() else {
            return;
        };
        let Some(index) = self
            .state
            .filtered_tracks()
            .iter()
            .position(|entry| entry.data.song.id == playing.id)
        else {
            return;
        };
        if index == self.state.browse.nav.track.sel() {
            return;
        }
        self.state.browse.nav.track.set_sel(index);
        self.state.browse.nav.last_sel_change = Instant::now();
    }

    /// 列表光标移动 forwarder(dispatch / scroll 走它);逻辑在 [`BrowsePage::move_selection`]。
    pub(in crate::app) fn move_selection(&mut self, mv: SelectionMove) {
        self.state.browse.move_selection(
            mv,
            BrowseModel {
                library: &self.state.library,
                cfg: &self.state.cfg,
            },
        );
    }

    /// `<C-d>` 族滚动按上下文路由:全屏滚歌词;浏览态滚当前列表——视口目标与光标同移
    /// n 行(vim `<C-d>` 语义,保持光标的屏上相对位置),文档首尾边界由渲染端统一钳。
    /// `<C-d>` 族滚动 forwarder(dispatch 走它);逻辑在 [`BrowsePage::scroll`]。
    pub(in crate::app) fn scroll(&mut self, step: ScrollStep) {
        let eff = self.state.browse.scroll(
            step,
            BrowseModel {
                library: &self.state.library,
                cfg: &self.state.cfg,
            },
        );
        self.apply_browse_effect(eff);
    }

    /// 在当前视图「进入」forwarder(dispatch 走它);逻辑在 [`BrowsePage::activate_selection`]。
    pub(in crate::app) fn activate_selection(&mut self) {
        let eff = self.state.browse.activate_selection(BrowseModel {
            library: &self.state.library,
            cfg: &self.state.cfg,
        });
        self.apply_browse_effect(eff);
    }

    /// 在当前视图「返回」forwarder(dispatch 走它);逻辑在 [`BrowsePage::back_or_clear_search`]。
    pub(in crate::app) fn back_or_clear_search(&mut self) {
        let eff = self.state.browse.back_or_clear_search(BrowseModel {
            library: &self.state.library,
            cfg: &self.state.cfg,
        });
        self.apply_browse_effect(eff);
    }

    /// 记当前 Library 光标位置 forwarder(Shift+Q 退出 / 进全屏等多入口走它);更新内存表后按
    /// persist 档落盘。逻辑在 [`BrowsePage::remember_track_pos`]。
    pub(in crate::app) fn remember_track_pos(&mut self) {
        let persist = self.state.browse.remember_track_pos(BrowseModel {
            library: &self.state.library,
            cfg: &self.state.cfg,
        });
        if persist {
            self.ui_prefs
                .save_track_positions(&self.state.browse.nav.track_pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use mineral_model::{PlaylistId, SourceKind};

    use crate::app::App;
    use crate::test_support::{app_with_library, app_with_library_probed};

    /// 喂一个 Press 键给 App(走真实事件入口 `handle_event`)。
    fn press(app: &mut App, code: KeyCode) {
        app.handle_event(&Event::Key(KeyEvent::new(code, KeyModifiers::empty())));
    }

    /// 给测试 App 热更 Library 过滤起播范围。
    fn set_filter_play_scope(app: &mut App, scope: &str) -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.lua");
        std::fs::write(
            &path,
            format!("return {{ tui = {{ behavior = {{ filter_play_scope = \"{scope}\" }} }} }}"),
        )?;
        let (cfg, warnings) = mineral_config::load(&path)?;
        assert!(warnings.is_empty(), "配置字段应被 schema 接受:{warnings:?}");
        app.apply_config(std::sync::Arc::new(cfg));
        Ok(())
    }

    /// 列表导航逐键回归:j/k/J/K/g/G 在 Library 与 Playlists 两个视图移动选中,
    /// 大跨步长 7、越界钳到首末行。
    #[test]
    fn j_k_navigate_in_playlists_and_library() -> color_eyre::Result<()> {
        // Library:10 首,从 0 起。
        let mut app = app_with_library(10, /*sel_track*/ 0)?;
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.state.browse.nav.track.sel(), 1, "j 下移一行");
        press(&mut app, KeyCode::Char('J'));
        assert_eq!(app.state.browse.nav.track.sel(), 8, "J 大跨下移 7 行");
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.state.browse.nav.track.sel(), 9, "下移越界钳到末行");
        press(&mut app, KeyCode::Char('K'));
        assert_eq!(app.state.browse.nav.track.sel(), 2, "K 大跨上移 7 行");
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.state.browse.nav.track.sel(), 1, "k 上移一行");
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.state.browse.nav.track.sel(), 0, "g 跳首行");
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.state.browse.nav.track.sel(), 9, "G 跳末行");

        // Playlists:3 张歌单,同一组键作用于 sel_playlist。
        let mut app = app_with_library(3, /*sel_track*/ 0)?;
        app.state
            .browse
            .view
            .switch_to(crate::runtime::state::View::Playlists);
        app.state.library.playlists = vec![
            crate::test_support::playlist_view("p1", "A", SourceKind::NETEASE, 1),
            crate::test_support::playlist_view("p2", "B", SourceKind::NETEASE, 1),
            crate::test_support::playlist_view("p3", "C", SourceKind::NETEASE, 1),
        ];
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.state.browse.nav.playlist.sel(), 1, "j 下移一行");
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.state.browse.nav.playlist.sel(), 2, "G 跳末行");
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.state.browse.nav.playlist.sel(), 1, "k 上移一行");
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.state.browse.nav.playlist.sel(), 0, "g 跳首行");
        Ok(())
    }

    /// `c` 在曲目列表里跳到在播曲;被过滤掉 / 在歌单面时不乱动。
    #[test]
    fn jump_to_current_moves_track_cursor() -> color_eyre::Result<()> {
        let mut app = app_with_library(/*len*/ 10, /*sel_track*/ 0)?;
        let playing = app
            .state
            .filtered_tracks()
            .iter()
            .nth(4)
            .map(|entry| entry.data.song.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 应有第 5 首"))?;
        app.state.playback.track = Some(playing);
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(app.state.browse.nav.track.sel(), 4, "c 跳到在播曲");

        // 在播曲被搜索滤掉:没有目标就留在原地。
        app.state.browse.search.tracks.set_query("absent");
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(app.state.browse.nav.track.sel(), 4, "命中不到就不动");

        // 歌单面没有在播概念:按了不动。
        app.state.browse.search.tracks.clear();
        app.state
            .browse
            .view
            .switch_to(crate::runtime::state::View::Playlists);
        app.state.browse.nav.playlist.set_sel(1);
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(app.state.browse.nav.playlist.sel(), 1, "歌单光标不动");
        assert_eq!(app.state.browse.nav.track.sel(), 4, "曲目光标也不变");
        Ok(())
    }

    /// Playlists 视图 `l` 进 Library;Library 视图 Enter 触发 atomic PlayQueue
    /// (TestClient no-op,断 view 流转与不 panic)。
    #[test]
    fn l_enters_library_enter_plays() -> color_eyre::Result<()> {
        let mut app = app_with_library(3, /*sel_track*/ 2)?;
        app.state
            .browse
            .view
            .switch_to(crate::runtime::state::View::Playlists);
        app.state.browse.nav.playlist.set_sel(0);

        press(&mut app, KeyCode::Char('l'));
        assert_eq!(
            app.state.browse.view,
            crate::runtime::state::View::Library,
            "l 应进 Library"
        );
        assert_eq!(
            app.state.browse.nav.track.sel(),
            0,
            "进 Library 选中复位到首行"
        );

        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.state.browse.view,
            crate::runtime::state::View::Library,
            "Enter 播放不切视图"
        );
        Ok(())
    }

    /// 默认 `collection`:过滤只定位,Enter 提交完整歌单并把 target 映射回原 occurrence。
    #[test]
    fn filtered_enter_defaults_to_full_collection() -> color_eyre::Result<()> {
        let (mut app, queue_ops) = app_with_library_probed(/*len*/ 3, /*sel_track*/ 0)?;
        let playlist_id = PlaylistId::new(SourceKind::NETEASE, "p1");
        let first_id = app
            .state
            .library
            .tracks
            .get(&playlist_id)
            .and_then(|entries| entries.first())
            .map(|entry| entry.data.song.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 应有首曲"))?;
        let third = app
            .state
            .library
            .tracks
            .get_mut(&playlist_id)
            .and_then(|entries| entries.get_mut(2))
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 应有第 3 首"))?;
        third.data.song.id = first_id;
        app.state.browse.search.tracks.set_query("Gjs");
        let want_id = app
            .state
            .filtered_tracks()
            .first()
            .map(|entry| entry.data.song.id.qualified())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 应过滤到 Gjs"))?;

        press(&mut app, KeyCode::Enter);

        let ops = queue_ops
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("queue_ops 锁中毒: {e}"))?;
        assert_eq!(
            *ops,
            vec![("play_queue", format!("3:2:{want_id}"))],
            "默认应提交完整 collection,target 映射到原第 2 个 occurrence"
        );
        Ok(())
    }

    /// `matches`:过滤态 Enter 只提交 filtered projection。
    #[test]
    fn filtered_enter_can_play_matches_only() -> color_eyre::Result<()> {
        let (mut app, queue_ops) = app_with_library_probed(/*len*/ 3, /*sel_track*/ 0)?;
        set_filter_play_scope(&mut app, "matches")?;
        app.state.browse.search.tracks.set_query("Gjs");
        let want_id = app
            .state
            .filtered_tracks()
            .first()
            .map(|entry| entry.data.song.id.qualified())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 应过滤到 Gjs"))?;

        press(&mut app, KeyCode::Enter);

        let ops = queue_ops
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("queue_ops 锁中毒: {e}"))?;
        assert_eq!(
            *ops,
            vec![("play_queue", format!("1:0:{want_id}"))],
            "matches 档应只提交过滤结果"
        );
        Ok(())
    }

    /// 搜索退格:逐字符删、删到空仍在搜索态,空 query 上再退格 = 退出搜索(vim 行为)。
    #[test]
    fn search_backspace_on_empty_query_exits() -> color_eyre::Result<()> {
        let mut app = app_with_library(3, /*sel_track*/ 0)?;

        // `/` 进入搜索输入态,query 起始为空。
        press(&mut app, KeyCode::Char('/'));
        assert!(app.state.browse.search.tracks.typing, "`/` 应进入搜索态");
        assert!(app.state.browse.search.tracks.query().is_empty());

        // 输入两个字符。
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(app.state.browse.search.tracks.query(), "ab");

        // 退格逐字符删;删到空时仍停在搜索态(不提前退出)。
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Backspace);
        assert!(app.state.browse.search.tracks.query().is_empty());
        assert!(
            app.state.browse.search.tracks.typing,
            "删到空时仍应在搜索态"
        );

        // 空 query 上再删一次 → 退出搜索。
        press(&mut app, KeyCode::Backspace);
        assert!(
            !app.state.browse.search.tracks.typing,
            "空 query 上退格应退出搜索"
        );
        Ok(())
    }

    /// 搜索输入态按光标位置插入字符，而不是固定追加到 query 末尾。
    #[test]
    fn search_cursor_edits_mid_query() -> color_eyre::Result<()> {
        let mut app = app_with_library(3, /*sel_track*/ 0)?;
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Char('b'));
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Char('X'));
        assert_eq!(
            app.state.browse.search.tracks.query(),
            "aXb",
            "左移后插入落在词中间"
        );
        Ok(())
    }

    /// `<C-f>`/`<C-b>` 浏览态翻页、`<C-d>`/`<C-u>` 单行档(步长随默认配置算,
    /// 调默认值不该改这条测试);全屏态同键路由去歌词,列表不动。
    #[test]
    fn ctrl_scroll_moves_cursor_in_browse() -> color_eyre::Result<()> {
        fn ctrl(app: &mut App, c: char) {
            app.handle_event(&Event::Key(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::CONTROL,
            )));
        }
        let mut app = crate::test_support::app_with_long_library(100, /*sel_track*/ 0)?;
        let page = *app.state.cfg.tui().behavior().page_scroll_rows();
        let line = *app.state.cfg.tui().behavior().line_scroll_rows();
        ctrl(&mut app, 'f');
        assert_eq!(
            app.state.browse.nav.track.sel(),
            page,
            "C-f 翻页下移 page_scroll_rows"
        );
        ctrl(&mut app, 'd');
        assert_eq!(
            app.state.browse.nav.track.sel(),
            page + line,
            "C-d 单行档下移 line_scroll_rows"
        );
        ctrl(&mut app, 'u');
        ctrl(&mut app, 'b');
        assert_eq!(app.state.browse.nav.track.sel(), 0, "C-u/C-b 对称滚回顶");
        ctrl(&mut app, 'b');
        assert_eq!(app.state.browse.nav.track.sel(), 0, "顶部再上滚钳住不越界");

        app.state.browse.fullscreen.set(true);
        ctrl(&mut app, 'f');
        assert_eq!(
            app.state.browse.nav.track.sel(),
            0,
            "全屏态 C-f 路由去歌词,列表光标不动"
        );
        Ok(())
    }

    /// 搜索输入态的 CONTROL 组合字符键(如 `<C-d>` 滚动)不得把裸字符泄漏进 query。
    #[test]
    fn search_ctrl_combos_do_not_leak_chars() -> color_eyre::Result<()> {
        fn ctrl(app: &mut App, c: char) {
            app.handle_event(&Event::Key(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::CONTROL,
            )));
        }
        let mut app = app_with_library(3, /*sel_track*/ 0)?;
        press(&mut app, KeyCode::Char('/'));
        for c in "ab".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        ctrl(&mut app, 'd');
        ctrl(&mut app, 'u');
        assert_eq!(
            app.state.browse.search.tracks.query(),
            "ab",
            "CONTROL 组合不进 query"
        );
        assert!(app.state.browse.search.tracks.typing, "也不退出搜索态");
        Ok(())
    }

    /// 深度命中定位:Playlists 视图搜到歌单内的歌后 Enter 进歌单,光标直接落在
    /// 命中歌上(而非第 0 行)。
    #[test]
    fn enter_on_deep_hit_locates_song_in_library() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;

        use crate::test_support::{entry_views, song, with_name};

        let (mut app, _submitted) = crate::test_support::app_with_playlists_probed()?;
        let pid = PlaylistId::new(SourceKind::NETEASE, "p2");
        let songs = ["甲", "乙", "春日影"]
            .into_iter()
            .map(|name| with_name(song(name), name))
            .collect();
        let views = entry_views(songs);
        app.state.library.tracks.insert(
            pid,
            crate::runtime::state::PlaylistTracks {
                entries: views,
                complete: true,
                next_offset: None,
            },
        );
        app.state.library.tracks_generation = 1;

        press(&mut app, KeyCode::Char('/'));
        for c in "春日影".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter); // 提交过滤,保留词
        press(&mut app, KeyCode::Enter); // activate 选中歌单
        assert_eq!(
            app.state.browse.view,
            crate::runtime::state::View::Library,
            "Enter 应进 Library"
        );
        assert_eq!(
            app.state.browse.nav.track.sel(),
            2,
            "光标应落在命中歌「春日影」上"
        );
        assert!(
            app.state.browse.search.tracks.query().is_empty(),
            "进入后展示完整曲目"
        );
        assert_eq!(
            app.state.browse.search.playlists.query(),
            "春日影",
            "父列表查询保留"
        );
        Ok(())
    }

    /// 配置旋钮:`search.deep.locate_on_enter = false` 时,深度命中行 Enter 进歌单
    /// 仍从第 0 行开始(不定位)。
    #[test]
    fn locate_on_enter_disabled_keeps_top() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;

        use crate::test_support::{entry_views, song, with_name};

        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.lua");
        std::fs::write(
            &path,
            "return { tui = { search = { deep = { locate_on_enter = false } } } }",
        )?;
        let (mut app, _submitted) = crate::test_support::app_with_playlists_probed()?;
        let (cfg, _warnings) = mineral_config::load(&path)?;
        app.apply_config(std::sync::Arc::new(cfg));
        let pid = PlaylistId::new(SourceKind::NETEASE, "p2");
        let songs = ["甲", "乙", "春日影"]
            .into_iter()
            .map(|name| with_name(song(name), name))
            .collect();
        let views = entry_views(songs);
        app.state.library.tracks.insert(
            pid,
            crate::runtime::state::PlaylistTracks {
                entries: views,
                complete: true,
                next_offset: None,
            },
        );
        app.state.library.tracks_generation = 1;

        press(&mut app, KeyCode::Char('/'));
        for c in "春日影".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.state.browse.nav.track.sel(),
            0,
            "旋钮关闭:不定位,从头看"
        );
        Ok(())
    }

    /// 深度搜索数据保障:Playlists 视图按 `/`,所有未缓存歌单的 PlaylistDetail 一次性
    /// 提交;再次进搜索态不重复提交(`tracks_requested` 去重);Library 视图按 `/` 不提交。
    #[test]
    fn slash_in_playlists_requests_uncached_tracks() -> color_eyre::Result<()> {
        use mineral_task::{ChannelFetchKind, TaskKind};

        let (mut app, submitted) = crate::test_support::app_with_playlists_probed()?;
        press(&mut app, KeyCode::Char('/'));
        {
            let tasks = submitted
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("探针锁中毒: {e}"))?;
            let track_fetches = tasks
                .iter()
                .filter(|k| {
                    matches!(
                        k,
                        TaskKind::ChannelFetch(ChannelFetchKind::PlaylistDetail { .. })
                    )
                })
                .count();
            assert_eq!(track_fetches, 3, "三个未缓存歌单各提交一次");
        }

        // 退出搜索再进:tracks_requested 已记录,不重复提交。
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('/'));
        {
            let tasks = submitted
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("探针锁中毒: {e}"))?;
            assert_eq!(tasks.len(), 3, "重复进搜索态不应重复提交");
        }

        // Library 视图按 `/` 不触发补拉(深度搜索只服务 Playlists 过滤)。
        press(&mut app, KeyCode::Esc);
        app.state
            .browse
            .view
            .switch_to(crate::runtime::state::View::Library);
        app.state.browse.nav.opened_playlist = Some(PlaylistId::new(SourceKind::NETEASE, "p1"));
        app.state.library.tracks_requested.clear();
        press(&mut app, KeyCode::Char('/'));
        let tasks = submitted
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("探针锁中毒: {e}"))?;
        assert_eq!(tasks.len(), 3, "Library 视图进搜索态不应提交补拉");
        Ok(())
    }

    /// 已有首批的歌单仍须加入深度搜索的完整加载范围。
    #[test]
    fn deep_search_upgrades_cached_previews() -> color_eyre::Result<()> {
        let (mut app, submitted) = crate::test_support::app_with_playlists_probed()?;
        let id = PlaylistId::new(SourceKind::NETEASE, "p1");
        app.state.library.tracks.insert(
            id.clone(),
            crate::runtime::state::PlaylistTracks {
                entries: crate::test_support::entry_views(vec![mineral_test::song("1")]),
                complete: false,
                next_offset: None,
            },
        );
        app.state
            .library
            .request_playlist(id.clone(), mineral_channel_core::PlaylistLoad::Preview);
        press(&mut app, KeyCode::Char('/'));
        let tasks = submitted
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("{e}"))?;
        assert!(tasks.iter().any(|kind| matches!(kind, mineral_task::TaskKind::ChannelFetch(mineral_task::ChannelFetchKind::PlaylistDetail { id: requested, load: mineral_channel_core::PlaylistLoad::Complete }) if requested == &id)));
        Ok(())
    }

    /// 配置总开关:`search.deep.enabled = false` 时按 `/` 不触发任何补拉。
    #[test]
    fn slash_respects_deep_disabled() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.lua");
        std::fs::write(
            &path,
            "return { tui = { search = { deep = { enabled = false } } } }",
        )?;
        let (mut app, submitted) = crate::test_support::app_with_playlists_probed()?;
        let (cfg, _warnings) = mineral_config::load(&path)?;
        app.apply_config(std::sync::Arc::new(cfg));
        press(&mut app, KeyCode::Char('/'));
        let tasks = submitted
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("探针锁中毒: {e}"))?;
        assert!(tasks.is_empty(), "deep 关闭时不应提交补拉");
        Ok(())
    }

    /// 位置记忆(默认 session 档):进歌单移动光标后退出再进,光标恢复原位
    /// (而非复位第 0 行),视口 snap 不残留动画。
    #[test]
    fn reenter_restores_remembered_position() -> color_eyre::Result<()> {
        let mut app = app_with_library(10, /*sel_track*/ 0)?;
        for _ in 0..3 {
            press(&mut app, KeyCode::Char('j'));
        }
        assert_eq!(app.state.browse.nav.track.sel(), 3, "前置:光标已下移");

        press(&mut app, KeyCode::Char('h'));
        assert_eq!(
            app.state.browse.view,
            crate::runtime::state::View::Playlists,
            "h 退回 Playlists"
        );
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(
            app.state.browse.nav.track.sel(),
            3,
            "再进同一歌单应恢复原位"
        );
        Ok(())
    }

    /// 相对位置记忆(端到端,经真实渲染):离开时光标在屏上第 N 行,再进恢复后
    /// 仍在第 N 行——而非统一顶到 scrolloff 位。
    #[test]
    fn reenter_restores_screen_relative_position() -> color_eyre::Result<()> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = crate::test_support::app_with_long_library(100, /*sel_track*/ 0)?;
        // sidebar 按 view 过渡端点选画哪个视图,推到 Library 端(at_max)。
        app.state
            .browse
            .view
            .switch_to(crate::runtime::state::View::Library);
        for _ in 0..40 {
            app.state.browse.view.tick();
        }
        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        app.state.browse.nav.track.set_sel(50);
        // 渲染若干帧让视口收敛(光标深处 → offset > 0,光标落在视口下安全边界)。
        for _ in 0..40 {
            t.draw(|f| crate::view::draw(f, &app))?;
        }
        let off = app.state.browse.nav.track.scroll_target();
        assert!(off > 0 && off <= 50, "前置:视口已滚到深处: {off}");
        let row_before = 50_usize.saturating_sub(off);

        press(&mut app, KeyCode::Char('h'));
        press(&mut app, KeyCode::Char('l'));
        for _ in 0..5 {
            t.draw(|f| crate::view::draw(f, &app))?;
        }
        assert_eq!(app.state.browse.nav.track.sel(), 50, "光标恢复原行");
        let row_after = 50_usize.saturating_sub(app.state.browse.nav.track.scroll_target());
        assert_eq!(row_after, row_before, "屏上相对行应与离开时一致");
        Ok(())
    }

    /// 旋钮关闭:`remember_track_pos = "off"` 时退出再进恒回第 0 行。
    #[test]
    fn remember_off_returns_to_top() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.lua");
        std::fs::write(
            &path,
            "return { tui = { behavior = { remember_track_pos = \"off\" } } }",
        )?;
        let mut app = app_with_library(10, /*sel_track*/ 0)?;
        let (cfg, _warnings) = mineral_config::load(&path)?;
        app.apply_config(std::sync::Arc::new(cfg));
        for _ in 0..3 {
            press(&mut app, KeyCode::Char('j'));
        }
        press(&mut app, KeyCode::Char('h'));
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.state.browse.nav.track.sel(), 0, "off 档不记不恢复");
        Ok(())
    }

    /// 双锚恢复:记忆锚定 song_id,歌单头部删一首后再进,光标仍指向同一首歌
    /// (下标顺移),而不是停在过时的旧下标上。
    #[test]
    fn memory_anchors_by_song_id_across_mutation() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;

        let mut app = app_with_library(10, /*sel_track*/ 0)?;
        for _ in 0..5 {
            press(&mut app, KeyCode::Char('j'));
        }
        press(&mut app, KeyCode::Char('h'));

        // 歌单第 0 首被删(模拟远端歌单变动后重新拉取)。
        let pid = PlaylistId::new(SourceKind::NETEASE, "p1");
        if let Some(tracks) = app.state.library.tracks.get_mut(&pid)
            && !tracks.is_empty()
        {
            tracks.remove(0);
        }
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(
            app.state.browse.nav.track.sel(),
            4,
            "同一首歌删行后顺移到 4"
        );
        Ok(())
    }

    /// 优先级:深度搜索命中定位压过记忆位置——记忆说在第 0 行,命中歌在第 2 行,
    /// 进歌单应落在命中歌上。
    #[test]
    fn deep_hit_beats_remembered_position() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;

        use crate::runtime::track_pos::TrackPos;
        use crate::test_support::{entry_views, song, with_name};

        let (mut app, _submitted) = crate::test_support::app_with_playlists_probed()?;
        let pid = PlaylistId::new(SourceKind::NETEASE, "p2");
        let songs = ["甲", "乙", "春日影"]
            .into_iter()
            .map(|name| with_name(song(name), name))
            .collect();
        let views = entry_views(songs);
        app.state.library.tracks.insert(
            pid.clone(),
            crate::runtime::state::PlaylistTracks {
                entries: views,
                complete: true,
                next_offset: None,
            },
        );
        app.state.library.tracks_generation = 1;
        app.state.browse.nav.track_pos.insert(
            pid,
            TrackPos {
                song_id: song("甲").id,
                index: 0,
                screen_row: 0,
            },
        );

        press(&mut app, KeyCode::Char('/'));
        for c in "春日影".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.state.browse.nav.track.sel(),
            2,
            "深度命中应压过记忆位置"
        );
        Ok(())
    }

    /// 曲目未就绪:进还没拉到曲目的歌单,先挂 pending(光标暂落 0),
    /// 不直接用过时下标硬恢复。
    #[test]
    fn enter_uncached_playlist_parks_pending_restore() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;

        use crate::runtime::track_pos::TrackPos;
        use crate::test_support::song;

        let (mut app, _submitted) = crate::test_support::app_with_playlists_probed()?;
        let pid = PlaylistId::new(SourceKind::NETEASE, "p1");
        app.state.browse.nav.track_pos.insert(
            pid.clone(),
            TrackPos {
                song_id: song("丙").id,
                index: 2,
                screen_row: 0,
            },
        );
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.state.browse.nav.track.sel(), 0, "曲目未到先停第 0 行");
        assert!(
            app.state
                .browse
                .nav
                .pending_track_restore
                .as_ref()
                .is_some_and(|p| p.playlist == pid),
            "应挂起该歌单的 pending 恢复"
        );

        // 退出再进别处之前,pending 不应泄漏到后续进入。
        press(&mut app, KeyCode::Char('h'));
        assert!(
            app.state.browse.nav.pending_track_restore.is_none(),
            "退出 Library 应作废 pending"
        );
        Ok(())
    }

    /// 全屏态屏蔽列表导航 + 搜索 `/`;
    #[test]
    fn fullscreen_blocks_nav_and_search() -> color_eyre::Result<()> {
        let mut app = app_with_library(5, /*sel_track*/ 2)?;
        press(&mut app, KeyCode::Char('z'));
        assert!(app.state.browse.fullscreen.on());

        // j / g 导航被吞,选中不变。
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.state.browse.nav.track.sel(), 2, "全屏屏蔽列表导航");

        // `/` 不进搜索态。
        press(&mut app, KeyCode::Char('/'));
        assert!(!app.state.browse.search.tracks.typing, "全屏屏蔽搜索 `/`");
        Ok(())
    }

    /// 全屏手动浏览(已脱离)时 Enter 跳到焦点行:seek 到该行时间戳 + 回附着态。
    /// 附着态(未脱离)时 Enter 不跳转也不改变状态(全屏无列表可 activate)。
    #[test]
    fn fullscreen_enter_seeks_to_focus_line() -> color_eyre::Result<()> {
        use crate::runtime::action::ScrollStep;
        use crate::test_support::app_in_fullscreen_seek_probe;

        let (mut app, seeks) = app_in_fullscreen_seek_probe()?;
        // 先注册当前歌(首个 tick 否则会因 scroll_song 未绑定而清脱离,与本测试无关)。
        app.state.tick_lyric_scroll();

        // 附着态:Enter 无跳转目标,不产生 seek。
        press(&mut app, KeyCode::Enter);
        assert!(
            seeks
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("seek 记录锁中毒: {e}"))?
                .is_empty(),
            "附着态 Enter 不 seek"
        );

        // 手动下滚脱离,取焦点行时间戳作预期。
        app.state.scroll_lyrics(ScrollStep::PageDown);
        let focus = app
            .state
            .manual_lyric_focus_line()
            .ok_or_else(|| color_eyre::eyre::eyre!("已脱离应有焦点行"))?;
        let expected = app
            .state
            .current_lines()
            .and_then(|lines| lines.get(focus).and_then(|l| l.time_ms))
            .ok_or_else(|| color_eyre::eyre::eyre!("焦点行应有时间戳"))?;

        press(&mut app, KeyCode::Enter);
        assert_eq!(
            *seeks
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("seek 记录锁中毒: {e}"))?,
            vec![expected],
            "Enter 跳到焦点行时间戳"
        );
        // seek 未落地(position 未变)时锚点钉在焦点行不清脱离——避免先跳回旧播放行。
        app.state.tick_lyric_scroll();
        assert!(
            app.state.browse.lyric_view.scroll.is_some(),
            "seek 落地前钉住锚点(不立即回附着)"
        );
        // 模拟 snapshot 回传:播放进入焦点行并越过交叉淡入窗口(elapsed ≥ scroll_ms)→ 下一
        // tick 无缝清脱离回附着(仅落地不越窗则仍钉住,避免 attached 重演行切入)。
        let scroll_ms = *app.state.cfg.tui().lyrics().scroll_ms();
        app.state.playback.position_ms = expected + scroll_ms;
        app.state.tick_lyric_scroll();
        assert!(
            app.state.browse.lyric_view.scroll.is_none(),
            "seek 落地并越过淡入窗口后无缝回附着态"
        );
        Ok(())
    }
}
