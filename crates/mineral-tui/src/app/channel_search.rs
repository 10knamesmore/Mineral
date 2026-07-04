//! Search 布局态的键盘输入执行器:token prompt 打字 / 结果列 / 详情面板,按 [`SearchFocus`] 分派。
//!
//! 行为长在 `impl SearchPage` 上(Page 自管 view 状态):吃按键、改自身态,把"要 App 做
//! 的副作用"作为 [`SearchEffect`] 意图**返回**;App 侧 [`App::handle_channel_search_key`] 就地构造
//! 只读 [`SearchCtx`]、调 `on_key`、再 [`App::apply_search_effect`] 落地——Page 不反手摸
//! `client` / `notifications`,故可脱离 App 单测(喂 KeyEvent、断言返回的意图)。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mineral_channel_core::ChannelCaps;
use mineral_model::{Album, SearchKind, Song, SourceKind};
use mineral_task::{ChannelFetchKind, Priority, SearchPayload, TaskKind};
use rustc_hash::FxHashMap;

use crate::components::toast::notifications::{TextTint, tinted_text_item};
use crate::runtime::action::{Action, ScrollStep, SelectionMove};
use crate::runtime::keymap::{Keymap, chord_from_event};
use crate::runtime::scroll::viewport::step_delta;
use crate::runtime::state::{
    ArtistSection, DetailData, EntityRef, PromptSegment, SearchFocus, SearchPage,
};

use super::App;
use super::page::Page;

/// Page 决策所需的只读跨页上下文(= React 的 props)。借用而非拥有,故 App 侧必须在调用点
/// **就地构造**(写明 `&self.state.caps` 等字段路径),不能抽成 `self.page_ctx()` 方法——那会整借
/// `self.state`、与 `&mut self.state.channel_search` 冲突。
#[derive(Clone, Copy)]
pub(crate) struct SearchCtx<'a> {
    /// 各 source 能力声明(决定可搜 source / kind 列表)。
    pub caps: &'a FxHashMap<SourceKind, ChannelCaps>,

    /// 键位表(面板动词自查表;prompt 仍走裸键)。
    pub keymap: &'a Keymap,

    /// 交互手感段(detail 简介滚动的逐行 / 翻页档步长来源)。
    pub behavior: &'a mineral_config::BehaviorConfig,

    /// detail 下钻 / 返回滑动拍数(App 从动画配置预算好传入)。
    pub sweep_ticks: u16,
}

/// Search 页吃完按键后吐给 App 的副作用意图;[`App::apply_search_effect`] 落地。
pub(crate) enum SearchEffect {
    /// 替换队列并起播(set_queue + play_song 两步)。
    PlayQueue {
        /// 替换进播放队列的曲目(整列结果 / 整列表)。
        queue: Vec<Song>,

        /// 起播曲目(也是 set_queue 的 target)。Box 平衡各变体大小。
        song: Box<Song>,
    },

    /// 提交一条首页搜索任务(User 优先级)。
    Submit {
        /// 目标 source。
        source: SourceKind,

        /// 目标 kind。
        kind: SearchKind,

        /// 查询词。
        query: String,
    },

    /// 懒分页预取下一页(`offset > 0`):光标近结果列底时按当前 source/kind/query 续拉。
    /// 与 [`Self::Submit`] 同走 Search 任务,只是 `page.offset` 非零——scheduler 按 offset 进
    /// dedup key,在途同 offset 自动并掉,无需 client 侧再设在途闸。
    FetchMore {
        /// 目标 source。
        source: SourceKind,

        /// 目标 kind。
        kind: SearchKind,

        /// 查询词(与首页同词,续拉同一桶)。
        query: String,

        /// 下一页 offset(= `next_offset`,页对齐:已请求页数 × limit)。
        offset: u32,
    },

    /// flash「kind 已切到 xxx」提示(切 source 致 kind 落首项时)。
    FlashKind(SearchKind),

    /// 非搜索动词回落全局 dispatch(transport / 退出确认等照常生效)。
    Dispatch(Action),

    /// 纯状态改动,无副作用。
    None,
}

impl App {
    /// Search 布局态按键入口:就地构造只读 [`SearchCtx`]、交 Page 吃键、再落地它吐回的意图。
    pub(super) fn handle_channel_search_key(&mut self, key: &KeyEvent) {
        let anim = self.state.cfg.tui().animation();
        let sweep_ticks =
            crate::render::anim::ticks16_from_ms(*anim.sweep_ms(), *anim.frame_tick_ms());
        // SearchCtx 就地构造:channel_search / caps / cfg 是 self.state 三个不相交字段,keymap 在
        // self 上,全 disjoint,借用检查器放行(故不能抽成取 ctx 的方法,那会整借 self.state)。
        let eff = self.state.channel_search.on_key(
            key,
            SearchCtx {
                caps: &self.state.caps,
                keymap: &self.keymap,
                behavior: self.state.cfg.tui().behavior(),
                sweep_ticks,
            },
        );
        self.apply_search_effect(eff);
    }

    /// 落地 Search 页吐回的副作用意图。Page 只产意图、不碰 `client` / `notifications`,全在此收口。
    fn apply_search_effect(&mut self, eff: SearchEffect) {
        match eff {
            SearchEffect::PlayQueue { queue, song } => {
                // 与 library / detail 起播一致:先建队列上下文,再起播选中曲(漏 play_song 会换队不响)。
                self.client.set_queue(queue, song.id.clone());
                self.client.play_song(*song);
            }
            SearchEffect::Submit {
                source,
                kind,
                query,
            } => {
                self.client.submit_task(
                    TaskKind::ChannelFetch(ChannelFetchKind::Search {
                        source,
                        kind,
                        query,
                        page: mineral_channel_core::Page::default(),
                    }),
                    Priority::User,
                );
                // 首页在飞：结果区显 searching spinner，到货（含 0 条）由 apply_page 清。
                self.state.channel_search.mark_loading(kind);
            }
            SearchEffect::FetchMore {
                source,
                kind,
                query,
                offset,
            } => {
                // 续拉用与首页一致的页大小(`Page::default().limit`):榨干推断与 next_offset
                // 页对齐都锚定同一 limit,混用页大小会让 offset↔页号换算错位;offset 进
                // dedup key,在途同页自动并掉。
                let limit = mineral_channel_core::Page::default().limit;
                self.client.submit_task(
                    TaskKind::ChannelFetch(ChannelFetchKind::Search {
                        source,
                        kind,
                        query,
                        page: mineral_channel_core::Page::new(offset, limit),
                    }),
                    Priority::User,
                );
            }
            SearchEffect::FlashKind(kind) => self.notifications.flash(tinted_text_item(
                format!("kind \u{2192} {}", kind.label()),
                TextTint::Normal,
            )),
            SearchEffect::Dispatch(action) => self.dispatch(action),
            SearchEffect::None => {}
        }
    }
}

impl Page for SearchPage {
    type Effect = SearchEffect;
    type Ctx<'a> = SearchCtx<'a>;

    /// Search 布局态按键入口:按当前输入焦点分派(prompt 文本输入 / 面板导航),返回副作用意图。
    fn on_key(&mut self, key: &KeyEvent, ctx: SearchCtx<'_>) -> SearchEffect {
        match self.focus {
            SearchFocus::Prompt => self.handle_search_prompt_key(key, ctx),
            SearchFocus::Results | SearchFocus::Detail => self.handle_search_panel_key(key, ctx),
        }
    }
}

impl SearchPage {
    /// 面板(results / detail)导航:搜索界面非文本输入,只截获面板导航,其余键回落全局
    /// dispatch（[`SearchEffect::Dispatch`]）——transport(播放/音量/seek/模式)、退出确认等照常生效。
    ///
    /// 截获:回 prompt 的模式键直拦;`activate` 前进、`back` 后退;`move_*` 移结果光标
    /// (仅结果列焦点;detail 焦点忽略——既不动 results 也不回落去动浏览列表)。
    fn handle_search_panel_key(&mut self, key: &KeyEvent, ctx: SearchCtx<'_>) -> SearchEffect {
        // Tab 回 prompt 是 search 布局态的模态逃逸:全局 Tab 绑 OpenQueue,扁平 keymap 无法让
        // 同一键在 search 内另作他用,故此处保留裸拦截;其余面板动词都走 keymap → Action。
        if key.code == KeyCode::Tab {
            self.set_focus(SearchFocus::Prompt);
            return SearchEffect::None;
        }
        match chord_from_event(key).and_then(|chord| ctx.keymap.lookup(chord)) {
            Some(Action::MoveSelection(mv)) => {
                self.move_search_panel(mv, *ctx.behavior.search_prefetch_rows())
            }
            Some(Action::ActivateSelection) => self.activate_search_panel(ctx.sweep_ticks),
            Some(Action::DrillIntoSelection) => {
                self.drill_search_panel(ctx.sweep_ticks);
                SearchEffect::None
            }
            Some(Action::CycleDetailSection) => {
                self.cycle_detail_section(ctx.sweep_ticks);
                SearchEffect::None
            }
            // detail 焦点下 C-d/u/b/f 滚头部简介(与列表光标 j/k 分治、键不重叠);其它焦点
            // 不接管(回落,与改前一致)。
            Some(Action::Scroll(step)) if self.focus == SearchFocus::Detail => {
                self.scroll_detail_description(step, ctx.behavior);
                SearchEffect::None
            }
            Some(Action::BackOrClearSearch) => {
                self.back_search_panel(ctx.sweep_ticks);
                SearchEffect::None
            }
            Some(other) => SearchEffect::Dispatch(other),
            None => SearchEffect::None,
        }
    }

    /// 面板前进一格(results → detail;detail 已是最右,无操作)。`activate` 绑定触发。
    fn focus_search_panel_forward(&mut self) {
        if self.focus == SearchFocus::Results {
            self.set_focus(SearchFocus::Detail);
        }
    }

    /// 面板导航:results 焦点移结果列(可能触发懒分页预取,故回传 effect)、detail 焦点移
    /// 当前区列表(无副作用)。
    ///
    /// # Params:
    ///   - `mv`: 选择移动
    ///   - `prefetch_rows`: 结果列预取触发半径(`behavior.search_prefetch_rows`)
    fn move_search_panel(&mut self, mv: SelectionMove, prefetch_rows: u16) -> SearchEffect {
        match self.focus {
            SearchFocus::Results => self.move_search_result_sel(mv, prefetch_rows),
            SearchFocus::Detail => {
                self.move_detail_list_sel(mv);
                SearchEffect::None
            }
            SearchFocus::Prompt => SearchEffect::None,
        }
    }

    /// detail 列表光标(钳当前区列表长度)。
    fn move_detail_list_sel(&mut self, mv: SelectionMove) {
        let Some(kr) = self.active_results_mut() else {
            return;
        };
        let Some(frame) = kr.detail.current_mut() else {
            return;
        };
        let len = frame.list_len();
        frame.list_mut().move_by(mv, len);
    }

    /// detail 焦点滚头部简介:按方向 + 档位(逐行 / 翻页)平移简介滚动 offset,档步长取
    /// `behavior`(与歌词 / 列表 / 队列滚动同源)。上界由 render 端按折行内容高度钳;
    /// 无活跃结果 / 无栈顶帧 → no-op。
    fn scroll_detail_description(
        &mut self,
        step: ScrollStep,
        behavior: &mineral_config::BehaviorConfig,
    ) {
        let delta = step_delta(step, behavior);
        if let Some(frame) = self.active_results().and_then(|kr| kr.detail.current()) {
            frame.nudge_description(delta);
        }
    }

    /// 面板激活(`activate`):results → 按实体做主事(song 播 / 容器开 detail);
    /// detail → 下钻专辑 / 替换队列播放选中曲。
    fn activate_search_panel(&mut self, sweep_ticks: u16) -> SearchEffect {
        match self.focus {
            SearchFocus::Results => self.activate_search_result(),
            SearchFocus::Detail => self.activate_detail_item(sweep_ticks),
            SearchFocus::Prompt => SearchEffect::None,
        }
    }

    /// 结果列 activate:选中行是 song(叶子)→ 替换队列播放(队列=整列结果);
    /// album/artist/playlist(容器)→ 进 detail 面板浏览(纯状态,无副作用)。
    fn activate_search_result(&mut self) -> SearchEffect {
        match self.result_play_target() {
            Some((queue, song)) => SearchEffect::PlayQueue {
                queue,
                song: Box::new(song),
            },
            None => {
                self.focus_search_panel_forward();
                SearchEffect::None
            }
        }
    }

    /// 结果列选中行若是 song,给出(整列队列, 选中曲);非 song 结果(容器)→ `None`。
    fn result_play_target(&self) -> Option<(Vec<Song>, Song)> {
        let kr = self.active_results()?;
        let SearchPayload::Songs(songs) = &kr.results else {
            return None;
        };
        let song = songs.get(kr.sel())?.clone();
        Some((songs.clone(), song))
    }

    /// 面板下探(`drill_into`):results → 进 detail(song 进其专辑、容器进详情);
    /// detail → 下钻选中专辑(歌手专辑区;曲目是叶子,无可下钻)。纯状态,无副作用。
    fn drill_search_panel(&mut self, sweep_ticks: u16) {
        match self.focus {
            SearchFocus::Results => self.focus_search_panel_forward(),
            SearchFocus::Detail => self.drill_detail_item(sweep_ticks),
            SearchFocus::Prompt => {}
        }
    }

    /// detail 下探:只取「下钻专辑」那支(歌手专辑区选中专辑 push 帧),曲目是叶子无操作。
    /// 复用 [`Self::detail_activate_action`] 的判定,与 `activate` 同源——activate 接 Drill+Play、
    /// drill 只接 Drill。
    fn drill_detail_item(&mut self, sweep_ticks: u16) {
        if let DetailActivate::Drill(album) = self.detail_activate_action()
            && let Some(kr) = self.active_results_mut()
        {
            kr.detail.push(EntityRef::Album(album), sweep_ticks);
        }
    }

    /// detail 激活:歌手专辑区选中 album → push 下钻帧(纯状态);其余列表选中 song → 替换队列播放。
    fn activate_detail_item(&mut self, sweep_ticks: u16) -> SearchEffect {
        match self.detail_activate_action() {
            DetailActivate::Drill(album) => {
                if let Some(kr) = self.active_results_mut() {
                    kr.detail.push(EntityRef::Album(album), sweep_ticks);
                }
                SearchEffect::None
            }
            DetailActivate::Play { queue, song } => SearchEffect::PlayQueue { queue, song },
            DetailActivate::None => SearchEffect::None,
        }
    }

    /// 读当前 detail 帧 + 选中项,定出激活动作(纯读,不改状态)。
    fn detail_activate_action(&self) -> DetailActivate {
        let Some(frame) = self.active_results().and_then(|kr| kr.detail.current()) else {
            return DetailActivate::None;
        };
        match (&frame.entity, frame.section, &frame.data) {
            // 歌手专辑区:选中专辑 → 下钻。
            (
                EntityRef::Artist(_),
                ArtistSection::Albums,
                Some(DetailData::Artist {
                    albums: Some(albs), ..
                }),
            ) => albs
                .get(frame.list().sel())
                .map_or(DetailActivate::None, |a| {
                    DetailActivate::Drill(Box::new(a.clone()))
                }),
            // 歌手热门曲:选中曲 → 播放。
            (
                EntityRef::Artist(_),
                ArtistSection::Hot,
                Some(DetailData::Artist {
                    detail: Some(a), ..
                }),
            ) => play_from(&a.songs, frame.list().sel()),
            // 专辑详情(专辑帧 / 歌曲帧看所属专辑)曲目 → 播放。
            (_, _, Some(DetailData::Album(a))) => play_from(&a.songs, frame.list().sel()),
            // 曲目列表(歌单帧)→ 播放。
            (_, _, Some(DetailData::Tracks(songs))) => play_from(songs, frame.list().sel()),
            _ => DetailActivate::None,
        }
    }

    /// 后退链:detail 先 pop 下钻帧;已在 root 则 detail → results、results → prompt。
    fn back_search_panel(&mut self, sweep_ticks: u16) {
        match self.focus {
            SearchFocus::Detail => {
                let popped = self
                    .active_results_mut()
                    .is_some_and(|kr| kr.detail.pop(sweep_ticks));
                if !popped {
                    self.set_focus(SearchFocus::Results);
                }
            }
            SearchFocus::Results => self.set_focus(SearchFocus::Prompt),
            SearchFocus::Prompt => {}
        }
    }

    /// 切歌手双区(仅歌手帧),光标归零并 arm 横向滑动。CycleDetailSection 经全局 keymap 派发、
    /// 任何焦点都可能到这里,仅 detail 焦点才动分区。
    fn cycle_detail_section(&mut self, sweep_ticks: u16) {
        if self.focus != SearchFocus::Detail {
            return;
        }
        let Some(kr) = self.active_results_mut() else {
            return;
        };
        let Some(frame) = kr.detail.current_mut() else {
            return;
        };
        if matches!(frame.entity, EntityRef::Artist(_)) {
            frame.cycle_section(sweep_ticks);
        }
    }

    /// 按一次 [`SelectionMove`] 移动当前会话结果列光标(钳首 / 末行),移动后按预取半径判是否
    /// 续拉下一页(回传 [`SearchEffect::FetchMore`])。
    ///
    /// # Params:
    ///   - `mv`: 选择移动
    ///   - `prefetch_rows`: 预取触发半径(光标距已加载末行 ≤ 此值且未榨干即预取)
    fn move_search_result_sel(&mut self, mv: SelectionMove, prefetch_rows: u16) -> SearchEffect {
        let Some(kr) = self.active_results_mut() else {
            return SearchEffect::None;
        };
        let last = kr.len().saturating_sub(1);
        let next = match mv {
            SelectionMove::Down(n) => kr.sel().saturating_add(n).min(last),
            SelectionMove::Up(n) => kr.sel().saturating_sub(n),
            SelectionMove::First => 0,
            SelectionMove::Last => last,
        };
        // set_sel 内联 detail 复位(真移动才复位、钳制不动则保留下钻栈)。
        kr.set_sel(next);
        // 预取:光标进入距已加载末行 prefetch_rows 行内、且桶未榨干 → 续拉 next_offset 那页。
        // 在途去重交给 scheduler(offset 进 dedup key),故移动即发、不在 client 设在途闸。
        // kr 派生量先落本地,释放可变借用,才能再不可变借 self 组 effect。
        let exhausted = kr.exhausted;
        let rows_to_bottom = last.saturating_sub(kr.sel());
        let next_offset = kr.next_offset;
        if exhausted || rows_to_bottom > usize::from(prefetch_rows) {
            return SearchEffect::None;
        }
        self.fetch_more_effect(next_offset)
    }

    /// 用当前 source/kind/query 组一条续拉(`offset > 0`)[`SearchEffect::FetchMore`];缺
    /// source / 会话 / query 空 → [`SearchEffect::None`]。
    fn fetch_more_effect(&self, offset: u32) -> SearchEffect {
        let Some(source) = self.source else {
            return SearchEffect::None;
        };
        let Some(session) = self.current() else {
            return SearchEffect::None;
        };
        if session.query().is_empty() {
            return SearchEffect::None;
        }
        SearchEffect::FetchMore {
            source,
            kind: session.kind,
            query: session.query().to_owned(),
            offset,
        }
    }

    /// token prompt 按键:按当前段（query 文本 / source·kind chip）分派。
    /// 带 CONTROL 的字符键吞掉(控制组合不污染 query / 不误触段切换)。
    fn handle_search_prompt_key(&mut self, key: &KeyEvent, ctx: SearchCtx<'_>) -> SearchEffect {
        if matches!(key.code, KeyCode::Char(_)) && key.modifiers.contains(KeyModifiers::CONTROL) {
            return SearchEffect::None;
        }
        match self.prompt_seg() {
            PromptSegment::Query => self.handle_query_seg_key(key, ctx),
            PromptSegment::Source | PromptSegment::Kind => self.handle_chip_seg_key(key, ctx),
        }
    }

    /// query 文本段:字符 / 退格 / 光标移动（词首再 left 跨到 kind chip）；Enter 提交搜索、
    /// Tab 回面板、Esc 退布局态。
    fn handle_query_seg_key(&mut self, key: &KeyEvent, ctx: SearchCtx<'_>) -> SearchEffect {
        match key.code {
            KeyCode::Esc => {
                self.active.toggle();
                SearchEffect::None
            }
            KeyCode::Enter => self.submit_search(),
            KeyCode::Tab => {
                let target = self.last_panel;
                self.set_focus(target);
                SearchEffect::None
            }
            KeyCode::Left => {
                self.query_seg_left(ctx);
                SearchEffect::None
            }
            KeyCode::Right => {
                self.move_prompt_cursor(SearchCursor::Right);
                SearchEffect::None
            }
            KeyCode::Home => {
                self.move_prompt_cursor(SearchCursor::Home);
                SearchEffect::None
            }
            KeyCode::End => {
                self.move_prompt_cursor(SearchCursor::End);
                SearchEffect::None
            }
            KeyCode::Backspace => {
                if let Some(session) = self.current_mut() {
                    session.pop_query_char();
                }
                SearchEffect::None
            }
            KeyCode::Char(c) => {
                if let Some(session) = self.current_mut() {
                    session.push_query_char(c);
                }
                SearchEffect::None
            }
            _ => SearchEffect::None,
        }
    }

    /// query 段 left:光标未到词首就左移;已在词首则跨到 kind chip 段（自动展开下拉）。
    fn query_seg_left(&mut self, ctx: SearchCtx<'_>) {
        let at_start = self.current().is_none_or(|s| s.query_split().0.is_empty());
        if at_start {
            self.focus_chip(PromptSegment::Kind, ctx);
        } else {
            self.move_prompt_cursor(SearchCursor::Left);
        }
    }

    /// 移动 token prompt 文本光标(无当前会话时 no-op)。
    fn move_prompt_cursor(&mut self, dir: SearchCursor) {
        let Some(session) = self.current_mut() else {
            return;
        };
        match dir {
            SearchCursor::Left => session.cursor_left(),
            SearchCursor::Right => session.cursor_right(),
            SearchCursor::Home => session.cursor_home(),
            SearchCursor::End => session.cursor_end(),
        }
    }

    /// source / kind chip 段:up/down 走查下拉、Enter 选定塌回（收起则重开）、left/right 段间走、
    /// Esc 先收下拉再退布局态、Tab 回面板。
    fn handle_chip_seg_key(&mut self, key: &KeyEvent, ctx: SearchCtx<'_>) -> SearchEffect {
        let seg = self.prompt_seg();
        match key.code {
            KeyCode::Esc => {
                if self.seg_open() {
                    self.close_seg();
                } else {
                    self.active.toggle();
                }
                SearchEffect::None
            }
            KeyCode::Tab => {
                let target = self.last_panel;
                self.set_focus(target);
                SearchEffect::None
            }
            KeyCode::Up => {
                self.move_seg_sel(seg, /*down*/ false, ctx);
                SearchEffect::None
            }
            KeyCode::Down => {
                self.move_seg_sel(seg, /*down*/ true, ctx);
                SearchEffect::None
            }
            KeyCode::Enter => self.chip_seg_enter(seg, ctx),
            KeyCode::Left => {
                self.chip_seg_left(seg, ctx);
                SearchEffect::None
            }
            KeyCode::Right => {
                self.chip_seg_right(seg, ctx);
                SearchEffect::None
            }
            _ => SearchEffect::None,
        }
    }

    /// 把 prompt 焦点移到某 chip 段:下拉自动展开,高亮落当前选择对应行。
    fn focus_chip(&mut self, seg: PromptSegment, ctx: SearchCtx<'_>) {
        let sel = self.chip_current_index(seg, ctx);
        self.set_prompt_seg(seg, sel);
    }

    /// 把 prompt 焦点移回 query 段:光标落词首（从 kind chip 右移进 query 即落词头）。
    fn focus_query(&mut self) {
        self.set_prompt_seg(PromptSegment::Query, 0);
        if let Some(session) = self.current_mut() {
            session.cursor_home();
        }
    }

    /// chip 下拉高亮行移动(钳列表范围;空列表 no-op)。
    fn move_seg_sel(&mut self, seg: PromptSegment, down: bool, ctx: SearchCtx<'_>) {
        let len = self.chip_options_len(seg, ctx);
        if len == 0 {
            return;
        }
        let cur = self.seg_sel();
        let next = if down {
            cur.saturating_add(1).min(len - 1)
        } else {
            cur.saturating_sub(1)
        };
        self.set_seg_sel(next);
    }

    /// chip 段 Enter:下拉展开时确认当前行（切 source / kind）并塌回;收起时重新展开。
    fn chip_seg_enter(&mut self, seg: PromptSegment, ctx: SearchCtx<'_>) -> SearchEffect {
        if !self.seg_open() {
            let sel = self.chip_current_index(seg, ctx);
            self.open_seg(sel);
            return SearchEffect::None;
        }
        let sel = self.seg_sel();
        let eff = match seg {
            PromptSegment::Source => self.confirm_source(sel, ctx),
            PromptSegment::Kind => self.confirm_kind(sel, ctx),
            PromptSegment::Query => SearchEffect::None,
        };
        self.close_seg();
        eff
    }

    /// 确认 source 选择:切到该 source（保留各 source 会话），首次进入吐 [`SearchEffect::FlashKind`];
    /// 焦点留在 source chip 段。
    fn confirm_source(&mut self, idx: usize, ctx: SearchCtx<'_>) -> SearchEffect {
        let Some(source) = self.source_options(ctx.caps).get(idx).copied() else {
            return SearchEffect::None;
        };
        match self.switch_source(source, ctx.caps) {
            Some(kind) => SearchEffect::FlashKind(kind),
            None => SearchEffect::None,
        }
    }

    /// 确认 kind 选择:切到该 kind;无缓存 + query 非空则用当前 query 自动搜（焦点留在 kind chip）。
    fn confirm_kind(&mut self, idx: usize, ctx: SearchCtx<'_>) -> SearchEffect {
        let Some(kind) = self.kind_options(ctx.caps).get(idx).copied() else {
            return SearchEffect::None;
        };
        if self.select_kind(kind) {
            self.submit_current_query()
        } else {
            SearchEffect::None
        }
    }

    /// chip 段 left:kind → source;source 已是最左,no-op。
    fn chip_seg_left(&mut self, seg: PromptSegment, ctx: SearchCtx<'_>) {
        if seg == PromptSegment::Kind {
            self.focus_chip(PromptSegment::Source, ctx);
        }
    }

    /// chip 段 right:source → kind;kind → query 文本段。
    fn chip_seg_right(&mut self, seg: PromptSegment, ctx: SearchCtx<'_>) {
        match seg {
            PromptSegment::Source => self.focus_chip(PromptSegment::Kind, ctx),
            PromptSegment::Kind => self.focus_query(),
            PromptSegment::Query => {}
        }
    }

    /// 某 chip 段当前选择在其列表里的下标（focus 到达时下拉高亮落此行）;找不到落 0。
    fn chip_current_index(&self, seg: PromptSegment, ctx: SearchCtx<'_>) -> usize {
        match seg {
            PromptSegment::Source => {
                let cur = self.source;
                self.source_options(ctx.caps)
                    .iter()
                    .position(|s| Some(*s) == cur)
                    .unwrap_or(0)
            }
            PromptSegment::Kind => {
                let cur = self.current().map(|s| s.kind);
                self.kind_options(ctx.caps)
                    .iter()
                    .position(|k| Some(*k) == cur)
                    .unwrap_or(0)
            }
            PromptSegment::Query => 0,
        }
    }

    /// 某 chip 段下拉的候选数(走查钳制用)。
    fn chip_options_len(&self, seg: PromptSegment, ctx: SearchCtx<'_>) -> usize {
        match seg {
            PromptSegment::Source => self.source_options(ctx.caps).len(),
            PromptSegment::Kind => self.kind_options(ctx.caps).len(),
            PromptSegment::Query => 0,
        }
    }

    /// 提交当前会话的首页搜索任务,焦点转结果列。空 query 不提交(留在 prompt、吐
    /// [`SearchEffect::None`])。显式提交即作废旧词缓存(per-kind 桶按当前 query 重建)。
    fn submit_search(&mut self) -> SearchEffect {
        let Some(source) = self.source else {
            return SearchEffect::None;
        };
        let (kind, query) = {
            let Some(session) = self.current_mut() else {
                return SearchEffect::None;
            };
            if session.query().is_empty() {
                return SearchEffect::None;
            }
            let pair = (session.kind, session.query().to_owned());
            session.clear_results();
            pair
        };
        self.set_focus(SearchFocus::Results);
        SearchEffect::Submit {
            source,
            kind,
            query,
        }
    }

    /// 用当前会话 source/kind/query 提交一次搜索（不改焦点、不清其它 kind 桶）——
    /// 切 kind 自动搜用,结果落桶后焦点仍留在 chip 段。
    fn submit_current_query(&self) -> SearchEffect {
        let Some(source) = self.source else {
            return SearchEffect::None;
        };
        let Some(session) = self.current() else {
            return SearchEffect::None;
        };
        if session.query().is_empty() {
            return SearchEffect::None;
        }
        SearchEffect::Submit {
            source,
            kind: session.kind,
            query: session.query().to_owned(),
        }
    }
}

/// token prompt 文本光标移动方向(Left/Right/Home/End 键映射)。
#[derive(Clone, Copy)]
enum SearchCursor {
    /// 左移一格。
    Left,

    /// 右移一格。
    Right,

    /// 跳词首。
    Home,

    /// 跳词尾。
    End,
}

/// detail 焦点 activate 的动作:下钻一张专辑、或替换队列播放选中曲。
enum DetailActivate {
    /// 歌手专辑区选中专辑 → push 下钻帧。
    Drill(Box<Album>),

    /// 列表选中曲 → 替换队列播放(队列 = 整个列表,起播 = 选中曲)。
    Play {
        /// 替换进播放队列的曲目(整列表)。
        queue: Vec<Song>,

        /// 起播曲目(也是 set_queue 的 target)。Box 平衡各变体大小。
        song: Box<Song>,
    },

    /// 无可激活项(列表空 / 数据未到)。
    None,
}

/// 从列表第 `sel` 首构造「替换队列播放」动作(队列 = 整个列表);越界为 None。
fn play_from(songs: &[Song], sel: usize) -> DetailActivate {
    match songs.get(sel) {
        Some(song) => DetailActivate::Play {
            queue: songs.to_vec(),
            song: Box::new(song.clone()),
        },
        None => DetailActivate::None,
    }
}

#[cfg(test)]
mod tests {
    use mineral_channel_core::Page;
    use mineral_model::{AlbumId, SearchKind, SourceKind};
    use mineral_task::{SearchPayload, TaskEvent};

    use super::DetailActivate;
    use crate::runtime::state::SearchFocus;

    /// 回归锁:专辑详情(`DetailData::Album`)里 activate 选中曲必须返回 `Play`(队列=专辑曲目、
    /// target=选中曲),而非静默 `None`。album 帧从 `Tracks` 改存 `Album` 后,
    /// `detail_activate_action` 曾漏补 `Album` 臂、落进 catch-all,导致专辑详情按播放键无反应。
    #[test]
    fn album_detail_activate_plays_selected_track() -> color_eyre::Result<()> {
        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Album])?;
        if let Some(s) = app.state.channel_search.current_mut() {
            s.set_query("q");
        }
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Album,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Albums(vec![
                mineral_model::Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                    .name("al1".to_owned())
                    .build(),
            ]),
            has_more: None,
        });
        let songs = crate::test_support::endserenading(4);
        let want = songs.get(2).map(|s| s.id.clone());
        app.state.apply(&TaskEvent::AlbumDetailFetched {
            id: AlbumId::new(SourceKind::NETEASE, "al1"),
            album: Box::new(
                mineral_model::Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                    .name("al1".to_owned())
                    .songs(songs)
                    .build(),
            ),
        });
        app.state.channel_search.set_focus(SearchFocus::Detail);
        // 光标移到第 3 首。
        if let Some(f) = app
            .state
            .channel_search
            .active_results_mut()
            .and_then(|kr| kr.detail.current_mut())
        {
            f.list_mut().set_sel(2);
        }
        match app.state.channel_search.detail_activate_action() {
            DetailActivate::Play { queue, song } => {
                assert_eq!(queue.len(), 4, "队列=专辑全部曲目");
                assert_eq!(Some(&song.id), want.as_ref(), "起播=选中第 3 首");
            }
            DetailActivate::Drill(_) => color_eyre::eyre::bail!("专辑详情曲目不应下钻"),
            DetailActivate::None => {
                color_eyre::eyre::bail!("回归:专辑详情 activate 落进 catch-all、静默无反应")
            }
        }
        Ok(())
    }

    /// 回归锁:search detail 曲目表必须用持久 offset + scrolloff(nvim 手感),不能每帧
    /// fresh `TableState` 把选中行钉死在视口底边。光标移到长列表深处后,选中行下方应仍留
    /// 至少 scrolloff 行——曾因 detail 列表绕过 `ListScroll` 而回归(焦点贴底)。
    #[test]
    fn detail_track_list_keeps_scrolloff_below_cursor() -> color_eyre::Result<()> {
        use mineral_model::{Album, AlbumId};
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Album])?;
        if let Some(s) = app.state.channel_search.current_mut() {
            s.set_query("q");
        }
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Album,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Albums(vec![
                Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                    .name("al1".to_owned())
                    .build(),
            ]),
            has_more: None,
        });
        // 30 首曲目的专辑详情(远超视口,光标可深入)。
        let songs = (0..30)
            .map(|i| crate::test_support::song(&format!("s{i}")))
            .collect::<Vec<_>>();
        app.state.apply(&TaskEvent::AlbumDetailFetched {
            id: AlbumId::new(SourceKind::NETEASE, "al1"),
            album: Box::new(
                Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                    .name("al1".to_owned())
                    .songs(songs)
                    .build(),
            ),
        });
        app.state.channel_search.set_focus(SearchFocus::Detail);
        // 光标深入到第 25 行(上下都还有曲目)。
        if let Some(f) = app
            .state
            .channel_search
            .active_results_mut()
            .and_then(|kr| kr.detail.current_mut())
        {
            f.list_mut().set_sel(25);
        }
        // 把 search 布局推到 at_max(detail 面板才渲染,而非浏览态)。
        app.state.channel_search.active.set(true);
        for _ in 0..40 {
            app.state.channel_search.active.tick();
        }
        let mut t = Terminal::new(TestBackend::new(120, 44))?;
        // 多帧渲染让视口缓动收敛到稳态。
        for _ in 0..16 {
            t.draw(|f| crate::view::draw(f, &app))?;
        }
        // detail 面板矩形(search 布局右栏)。
        let detail = crate::components::layout::shared::compute::compute_search(
            app.state.frame_area.get(),
            app.state.cfg.tui().layout(),
        )
        .right
        .ok_or_else(|| color_eyre::eyre::eyre!("search 布局应有 detail 面板"))?;
        let buf = t.backend().buffer();
        // 在 detail 面板 x 区间内找高亮行(highlight_symbol 前缀 '▌')。
        let mut hi_y: Option<u16> = None;
        'scan: for y in detail.y..detail.y.saturating_add(detail.height) {
            for x in detail.x..detail.x.saturating_add(detail.width) {
                if buf.cell((x, y)).is_some_and(|c| c.symbol() == "▌") {
                    hi_y = Some(y);
                    break 'scan;
                }
            }
        }
        let hi_y = hi_y.ok_or_else(|| color_eyre::eyre::eyre!("detail 面板应有高亮选中行"))?;
        let scrolloff = u16::try_from(app.state.scrolloff()).unwrap_or(0);
        // 列表填满到面板底(30 首 > 视口),底部数据行 ≈ 面板内底(去下边框)。
        let bottom_data = detail.y.saturating_add(detail.height).saturating_sub(2);
        assert!(
            hi_y.saturating_add(scrolloff) <= bottom_data,
            "选中行下方应留 ≥ scrolloff({scrolloff}) 行: hi_y={hi_y} bottom={bottom_data}"
        );
        Ok(())
    }

    /// 复现用户路径:artist 详情 → Albums 区 → 下钻进某专辑 → 在专辑帧 activate 选中曲应播放。
    #[test]
    fn artist_drilled_album_activate_plays() -> color_eyre::Result<()> {
        use mineral_model::{Album, ArtistId};

        use crate::runtime::state::ArtistSection;

        let (mut app, queue_ops) =
            crate::test_support::app_with_channel_search_qprobed(vec![SearchKind::Artist])?;
        if let Some(s) = app.state.channel_search.current_mut() {
            s.set_query("q");
        }
        let artist = mineral_model::Artist::builder()
            .id(ArtistId::new(SourceKind::NETEASE, "ar"))
            .name("ar".to_owned())
            .build();
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Artist,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Artists(vec![artist.clone()]),
            has_more: None,
        });
        let aid = ArtistId::new(SourceKind::NETEASE, "ar");
        app.state.apply(&TaskEvent::ArtistDetailFetched {
            id: aid.clone(),
            artist: Box::new(artist),
        });
        app.state.apply(&TaskEvent::ArtistAlbumsFetched {
            id: aid,
            page: Page::default(),
            albums: vec![
                Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                    .name("al1".to_owned())
                    .build(),
            ],
        });
        app.state.channel_search.set_focus(SearchFocus::Detail);
        // 切到 Albums 区,下钻进 al1。
        if let Some(f) = app
            .state
            .channel_search
            .active_results_mut()
            .and_then(|kr| kr.detail.current_mut())
        {
            f.section = ArtistSection::Albums;
        }
        let eff = app
            .state
            .channel_search
            .activate_detail_item(/*sweep_ticks*/ 1);
        app.apply_search_effect(eff);
        assert_eq!(
            app.state
                .channel_search
                .active_results()
                .map(|kr| kr.detail.depth()),
            Some(1),
            "应下钻进专辑帧"
        );
        // 专辑详情到货(带曲目)。
        let songs = crate::test_support::endserenading(3);
        let want = songs.get(1).map(|s| s.id.clone());
        app.state.apply(&TaskEvent::AlbumDetailFetched {
            id: AlbumId::new(SourceKind::NETEASE, "al1"),
            album: Box::new(
                Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                    .name("al1".to_owned())
                    .songs(songs)
                    .build(),
            ),
        });
        if let Some(f) = app
            .state
            .channel_search
            .active_results_mut()
            .and_then(|kr| kr.detail.current_mut())
        {
            f.list_mut().set_sel(1);
        }
        // 走完整 handler:不只是返回动作,而要真发出 set_queue + play_song 两步。
        let eff = app
            .state
            .channel_search
            .activate_detail_item(/*sweep_ticks*/ 1);
        app.apply_search_effect(eff);
        let ops = queue_ops
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("queue_ops 锁中毒: {e}"))?;
        let want_q = want
            .as_ref()
            .map(mineral_model::SongId::qualified)
            .unwrap_or_default();
        assert!(
            ops.iter()
                .any(|(op, arg)| *op == "set_queue" && arg == &format!("3:{want_q}")),
            "应 set_queue(队列=3 曲、target=选中曲):{ops:?}"
        );
        assert!(
            ops.iter()
                .any(|(op, arg)| *op == "play_song" && arg == &want_q),
            "回归:detail 起播必须 play_song(漏掉则队列换了却不响):{ops:?}"
        );
        Ok(())
    }

    /// 搜 song 时结果列 activate 直接播放选中那首(队列=整列结果),不进 detail——
    /// result 本身就是可播放的 song,叶子的主事就是播。
    #[test]
    fn song_result_activate_plays_selected() -> color_eyre::Result<()> {
        use mineral_model::SongId;

        let (mut app, queue_ops) =
            crate::test_support::app_with_channel_search_qprobed(vec![SearchKind::Song])?;
        if let Some(s) = app.state.channel_search.current_mut() {
            s.set_query("q");
        }
        let songs = crate::test_support::endserenading(4);
        let want = songs.get(2).map(|s| s.id.clone());
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Song,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Songs(songs),
            has_more: None,
        });
        app.state.channel_search.set_focus(SearchFocus::Results);
        if let Some(kr) = app.state.channel_search.active_results_mut() {
            kr.set_sel(2);
        }
        let eff = app
            .state
            .channel_search
            .activate_search_panel(/*sweep_ticks*/ 1);
        app.apply_search_effect(eff);
        let want_q = want.as_ref().map(SongId::qualified).unwrap_or_default();
        let ops = queue_ops
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("queue_ops 锁中毒: {e}"))?;
        assert!(
            ops.iter()
                .any(|(op, arg)| *op == "set_queue" && arg == &format!("4:{want_q}")),
            "song 结果 activate 应 set_queue(队列=4 条、target=选中第 3 首):{ops:?}"
        );
        assert!(
            ops.iter()
                .any(|(op, arg)| *op == "play_song" && arg == &want_q),
            "song 结果 activate 应直接 play_song 选中曲:{ops:?}"
        );
        assert_eq!(
            app.state.channel_search.focus,
            SearchFocus::Results,
            "song 直接播,焦点不进 detail"
        );
        Ok(())
    }

    /// 搜 album(容器)时结果列 activate 进 detail 浏览,不直接播放。
    #[test]
    fn container_result_activate_opens_detail() -> color_eyre::Result<()> {
        let (mut app, queue_ops) =
            crate::test_support::app_with_channel_search_qprobed(vec![SearchKind::Album])?;
        if let Some(s) = app.state.channel_search.current_mut() {
            s.set_query("q");
        }
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Album,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Albums(vec![
                mineral_model::Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                    .name("al1".to_owned())
                    .build(),
            ]),
            has_more: None,
        });
        app.state.channel_search.set_focus(SearchFocus::Results);
        let eff = app
            .state
            .channel_search
            .activate_search_panel(/*sweep_ticks*/ 1);
        app.apply_search_effect(eff);
        assert_eq!(
            app.state.channel_search.focus,
            SearchFocus::Detail,
            "容器结果 activate → 进 detail 浏览"
        );
        assert!(
            queue_ops
                .lock()
                .map_err(|e| color_eyre::eyre::eyre!("queue_ops 锁中毒: {e}"))?
                .is_empty(),
            "容器结果不直接播放,无队列操作"
        );
        Ok(())
    }

    /// detail 焦点下 C-f/C-b 走完整键路由滚动头部简介(平移 desc_scroll);与列表光标 j/k
    /// 分治、键不重叠。回归锁:Scroll 在 detail 焦点必须被 search 拦截作用于简介,而非穿透
    /// 全局 dispatch 去滚别处。
    #[test]
    fn detail_focus_ctrl_f_scrolls_description() -> color_eyre::Result<()> {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let (mut app, _q) =
            crate::test_support::app_with_channel_search_qprobed(vec![SearchKind::Album])?;
        if let Some(s) = app.state.channel_search.current_mut() {
            s.set_query("q");
        }
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Album,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Albums(vec![
                mineral_model::Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                    .name("al1".to_owned())
                    .build(),
            ]),
            has_more: None,
        });
        app.state.apply(&TaskEvent::AlbumDetailFetched {
            id: AlbumId::new(SourceKind::NETEASE, "al1"),
            album: Box::new(
                mineral_model::Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                    .name("al1".to_owned())
                    .description("line1\nline2\nline3".to_owned())
                    .songs(crate::test_support::endserenading(3))
                    .build(),
            ),
        });
        app.state.channel_search.set_focus(SearchFocus::Detail);
        let page = u16::try_from(*app.state.cfg.tui().behavior().page_scroll_rows())?;
        // C-f 翻页下滚简介(平移 page_scroll_rows)。
        app.handle_channel_search_key(&KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
        assert_eq!(
            app.state
                .channel_search
                .active_results()
                .and_then(|kr| kr.detail.current())
                .map(|f| f.description_scroll().get()),
            Some(page),
            "C-f 平移简介 offset = page_scroll_rows"
        );
        // C-b 回滚,下界钳 0。
        app.handle_channel_search_key(&KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(
            app.state
                .channel_search
                .active_results()
                .and_then(|kr| kr.detail.current())
                .map(|f| f.description_scroll().get()),
            Some(0),
            "C-b 回滚到顶(下界钳 0)"
        );
        Ok(())
    }

    /// drill_into 在结果列对任意实体都进 detail(song 进其专辑、容器进详情)。
    #[test]
    fn drill_from_results_enters_detail() -> color_eyre::Result<()> {
        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        if let Some(s) = app.state.channel_search.current_mut() {
            s.set_query("q");
        }
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Song,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Songs(crate::test_support::endserenading(3)),
            has_more: None,
        });
        app.state.channel_search.set_focus(SearchFocus::Results);
        app.state
            .channel_search
            .drill_search_panel(/*sweep_ticks*/ 1);
        assert_eq!(
            app.state.channel_search.focus,
            SearchFocus::Detail,
            "drill 在结果列 → 进 detail(song 进其专辑)"
        );
        Ok(())
    }
}
