//! Search 布局态的键盘输入执行器:token prompt 打字 / 结果列 / 详情面板,按 [`SearchFocus`] 分派。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mineral_channel_core::Page;
use mineral_model::{Album, SearchKind, Song, SourceKind};
use mineral_task::{ChannelFetchKind, Priority, SearchPayload, TaskKind};

use crate::components::toast::notifications::{TextTint, tinted_text_item};
use crate::runtime::action::{Action, SelectionMove};
use crate::runtime::keymap::chord_from_event;
use crate::runtime::state::{ArtistSection, DetailData, EntityRef, PromptSegment, SearchFocus};

use super::App;

impl App {
    /// Search 布局态按键入口:按当前输入焦点分派(prompt 文本输入 / 面板导航)。
    pub(super) fn handle_channel_search_key(&mut self, key: &KeyEvent) {
        match self.state.channel_search.focus {
            SearchFocus::Prompt => self.handle_search_prompt_key(key),
            SearchFocus::Results | SearchFocus::Detail => self.handle_search_panel_key(key),
        }
    }

    /// 面板(results / detail)导航:搜索界面非文本输入,只截获面板导航,其余键回落全局
    /// dispatch——transport(播放/音量/seek/模式)、退出确认等照常生效。
    ///
    /// 截获:回 prompt 的模式键直拦;`activate` 前进、`back` 后退;`move_*` 移结果光标
    /// (仅结果列焦点;detail 焦点忽略——既不动 results 也不回落去动浏览列表)。
    fn handle_search_panel_key(&mut self, key: &KeyEvent) {
        // Tab 回 prompt 是 search 布局态的模态逃逸:全局 Tab 绑 OpenQueue,扁平 keymap 无法让
        // 同一键在 search 内另作他用,故此处保留裸拦截;其余面板动词都走 keymap → Action。
        if key.code == KeyCode::Tab {
            self.state.channel_search.set_focus(SearchFocus::Prompt);
            return;
        }
        match chord_from_event(key).and_then(|chord| self.keymap.lookup(chord)) {
            Some(Action::MoveSelection(mv)) => self.move_search_panel(mv),
            Some(Action::ActivateSelection) => self.activate_search_panel(),
            Some(Action::DrillIntoSelection) => self.drill_search_panel(),
            Some(Action::CycleDetailSection) => self.cycle_detail_section(),
            Some(Action::BackOrClearSearch) => self.back_search_panel(),
            Some(other) => self.dispatch(other),
            None => {}
        }
    }

    /// 面板前进一格(results → detail;detail 已是最右,无操作)。`activate` 绑定触发。
    fn focus_search_panel_forward(&mut self) {
        if self.state.channel_search.focus == SearchFocus::Results {
            self.state.channel_search.set_focus(SearchFocus::Detail);
        }
    }

    /// 面板导航:results 焦点移结果列、detail 焦点移当前区列表。
    fn move_search_panel(&mut self, mv: SelectionMove) {
        match self.state.channel_search.focus {
            SearchFocus::Results => self.move_search_result_sel(mv),
            SearchFocus::Detail => self.move_detail_list_sel(mv),
            SearchFocus::Prompt => {}
        }
    }

    /// detail 列表光标(钳当前区列表长度)。
    fn move_detail_list_sel(&mut self, mv: SelectionMove) {
        let Some(kr) = self.state.channel_search.active_results_mut() else {
            return;
        };
        let Some(frame) = kr.detail.current_mut() else {
            return;
        };
        let last = frame.list_len().saturating_sub(1);
        frame.list_sel = match mv {
            SelectionMove::Down(n) => frame.list_sel.saturating_add(n).min(last),
            SelectionMove::Up(n) => frame.list_sel.saturating_sub(n),
            SelectionMove::First => 0,
            SelectionMove::Last => last,
        };
    }

    /// 面板激活(`activate`):results → 按实体做主事(song 播 / 容器开 detail);
    /// detail → 下钻专辑 / 替换队列播放选中曲。
    fn activate_search_panel(&mut self) {
        match self.state.channel_search.focus {
            SearchFocus::Results => self.activate_search_result(),
            SearchFocus::Detail => self.activate_detail_item(),
            SearchFocus::Prompt => {}
        }
    }

    /// 结果列 activate:选中行是 song(叶子)→ 直接替换队列播放(队列=整列结果);
    /// album/artist/playlist(容器)→ 进 detail 面板浏览。
    fn activate_search_result(&mut self) {
        match self.result_play_target() {
            Some((queue, song)) => {
                // 与 library / detail 起播一致:先建队列上下文,再起播选中曲(漏 play_song 会换队不响)。
                self.client.set_queue(queue, song.id.clone());
                self.client.play_song(song);
            }
            None => self.focus_search_panel_forward(),
        }
    }

    /// 结果列选中行若是 song,给出(整列队列, 选中曲);非 song 结果(容器)→ `None`。
    fn result_play_target(&self) -> Option<(Vec<Song>, Song)> {
        let kr = self.state.channel_search.active_results()?;
        let SearchPayload::Songs(songs) = &kr.results else {
            return None;
        };
        let song = songs.get(kr.sel)?.clone();
        Some((songs.clone(), song))
    }

    /// 面板下探(`drill_into`):results → 进 detail(song 进其专辑、容器进详情);
    /// detail → 下钻选中专辑(歌手专辑区;曲目是叶子,无可下钻)。
    fn drill_search_panel(&mut self) {
        match self.state.channel_search.focus {
            SearchFocus::Results => self.focus_search_panel_forward(),
            SearchFocus::Detail => self.drill_detail_item(),
            SearchFocus::Prompt => {}
        }
    }

    /// detail 下探:只取「下钻专辑」那支(歌手专辑区选中专辑 push 帧),曲目是叶子无操作。
    /// 复用 [`Self::detail_activate_action`] 的判定,与 `activate` 同源——activate 接 Drill+Play、
    /// drill 只接 Drill。
    fn drill_detail_item(&mut self) {
        if let DetailActivate::Drill(album) = self.detail_activate_action() {
            let ticks = self.detail_sweep_ticks();
            if let Some(kr) = self.state.channel_search.active_results_mut() {
                kr.detail.push(EntityRef::Album(album), ticks);
            }
        }
    }

    /// detail 激活:歌手专辑区选中 album → push 下钻帧;其余列表选中 song → 替换队列播放。
    fn activate_detail_item(&mut self) {
        match self.detail_activate_action() {
            DetailActivate::Drill(album) => {
                let ticks = self.detail_sweep_ticks();
                if let Some(kr) = self.state.channel_search.active_results_mut() {
                    kr.detail.push(EntityRef::Album(album), ticks);
                }
            }
            DetailActivate::Play { queue, song } => {
                // 两步,对齐 library 起播(nav.rs):先建队列上下文,再起播选中曲——
                // set_queue 只换队列不播,漏掉 play_song 会"队列换了却不响"。
                self.client.set_queue(queue, song.id.clone());
                self.client.play_song(*song);
            }
            DetailActivate::None => {}
        }
    }

    /// 读当前 detail 帧 + 选中项,定出激活动作(纯读,不改状态)。
    fn detail_activate_action(&self) -> DetailActivate {
        let Some(frame) = self
            .state
            .channel_search
            .active_results()
            .and_then(|kr| kr.detail.current())
        else {
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
            ) => albs.get(frame.list_sel).map_or(DetailActivate::None, |a| {
                DetailActivate::Drill(Box::new(a.clone()))
            }),
            // 歌手热门曲:选中曲 → 播放。
            (
                EntityRef::Artist(_),
                ArtistSection::Hot,
                Some(DetailData::Artist {
                    detail: Some(a), ..
                }),
            ) => play_from(&a.songs, frame.list_sel),
            // 专辑详情(专辑帧 / 歌曲帧看所属专辑)曲目 → 播放。
            (_, _, Some(DetailData::Album(a))) => play_from(&a.songs, frame.list_sel),
            // 曲目列表(歌单帧)→ 播放。
            (_, _, Some(DetailData::Tracks(songs))) => play_from(songs, frame.list_sel),
            _ => DetailActivate::None,
        }
    }

    /// 后退链:detail 先 pop 下钻帧;已在 root 则 detail → results、results → prompt。
    fn back_search_panel(&mut self) {
        match self.state.channel_search.focus {
            SearchFocus::Detail => {
                let ticks = self.detail_sweep_ticks();
                let popped = self
                    .state
                    .channel_search
                    .active_results_mut()
                    .is_some_and(|kr| kr.detail.pop(ticks));
                if !popped {
                    self.state.channel_search.set_focus(SearchFocus::Results);
                }
            }
            SearchFocus::Results => self.state.channel_search.set_focus(SearchFocus::Prompt),
            SearchFocus::Prompt => {}
        }
    }

    /// 切歌手双区(仅歌手帧),光标归零。CycleDetailSection 经全局 keymap 派发、任何焦点都可能
    /// 到这里,仅 detail 焦点才动分区。
    fn cycle_detail_section(&mut self) {
        if self.state.channel_search.focus != SearchFocus::Detail {
            return;
        }
        let Some(kr) = self.state.channel_search.active_results_mut() else {
            return;
        };
        let Some(frame) = kr.detail.current_mut() else {
            return;
        };
        if matches!(frame.entity, EntityRef::Artist(_)) {
            frame.section.cycle();
            frame.list_sel = 0;
        }
    }

    /// detail 下钻/返回滑动拍数(沿用 sidebar sweep 时长)。
    fn detail_sweep_ticks(&self) -> u16 {
        let anim = self.state.cfg.tui().animation();
        crate::render::anim::ticks16_from_ms(*anim.sweep_ms(), *anim.frame_tick_ms())
    }

    /// 按一次 [`SelectionMove`] 移动当前会话结果列光标(钳首 / 末行)。
    fn move_search_result_sel(&mut self, mv: SelectionMove) {
        let Some(kr) = self.state.channel_search.active_results_mut() else {
            return;
        };
        let last = kr.len().saturating_sub(1);
        let next = match mv {
            SelectionMove::Down(n) => kr.sel.saturating_add(n).min(last),
            SelectionMove::Up(n) => kr.sel.saturating_sub(n),
            SelectionMove::First => 0,
            SelectionMove::Last => last,
        };
        // set_sel 内联 detail 复位（真移动才复位、钳制不动则保留下钻栈）。
        kr.set_sel(next);
    }

    /// token prompt 按键:按当前段（query 文本 / source·kind chip）分派。
    /// 带 CONTROL 的字符键吞掉(控制组合不污染 query / 不误触段切换)。
    fn handle_search_prompt_key(&mut self, key: &KeyEvent) {
        if matches!(key.code, KeyCode::Char(_)) && key.modifiers.contains(KeyModifiers::CONTROL) {
            return;
        }
        match self.state.channel_search.prompt_seg() {
            PromptSegment::Query => self.handle_query_seg_key(key),
            PromptSegment::Source | PromptSegment::Kind => self.handle_chip_seg_key(key),
        }
    }

    /// query 文本段:字符 / 退格 / 光标移动（词首再 left 跨到 kind chip）；Enter 提交搜索、
    /// Tab 回面板、Esc 退布局态。
    fn handle_query_seg_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Esc => self.state.channel_search.active.toggle(),
            KeyCode::Enter => self.submit_search(),
            KeyCode::Tab => {
                let target = self.state.channel_search.last_panel;
                self.state.channel_search.set_focus(target);
            }
            KeyCode::Left => self.query_seg_left(),
            KeyCode::Right => self.move_prompt_cursor(SearchCursor::Right),
            KeyCode::Home => self.move_prompt_cursor(SearchCursor::Home),
            KeyCode::End => self.move_prompt_cursor(SearchCursor::End),
            KeyCode::Backspace => {
                if let Some(session) = self.state.channel_search.current_mut() {
                    session.pop_query_char();
                }
            }
            KeyCode::Char(c) => {
                if let Some(session) = self.state.channel_search.current_mut() {
                    session.push_query_char(c);
                }
            }
            _ => {}
        }
    }

    /// query 段 left:光标未到词首就左移;已在词首则跨到 kind chip 段（自动展开下拉）。
    fn query_seg_left(&mut self) {
        let at_start = self
            .state
            .channel_search
            .current()
            .is_none_or(|s| s.query_split().0.is_empty());
        if at_start {
            self.focus_chip(PromptSegment::Kind);
        } else {
            self.move_prompt_cursor(SearchCursor::Left);
        }
    }

    /// 移动 token prompt 文本光标(无当前会话时 no-op)。
    fn move_prompt_cursor(&mut self, dir: SearchCursor) {
        let Some(session) = self.state.channel_search.current_mut() else {
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
    fn handle_chip_seg_key(&mut self, key: &KeyEvent) {
        let seg = self.state.channel_search.prompt_seg();
        match key.code {
            KeyCode::Esc => {
                if self.state.channel_search.seg_open() {
                    self.state.channel_search.close_seg();
                } else {
                    self.state.channel_search.active.toggle();
                }
            }
            KeyCode::Tab => {
                let target = self.state.channel_search.last_panel;
                self.state.channel_search.set_focus(target);
            }
            KeyCode::Up => self.move_seg_sel(seg, /*down*/ false),
            KeyCode::Down => self.move_seg_sel(seg, /*down*/ true),
            KeyCode::Enter => self.chip_seg_enter(seg),
            KeyCode::Left => self.chip_seg_left(seg),
            KeyCode::Right => self.chip_seg_right(seg),
            _ => {}
        }
    }

    /// 把 prompt 焦点移到某 chip 段:下拉自动展开,高亮落当前选择对应行。
    fn focus_chip(&mut self, seg: PromptSegment) {
        let sel = self.chip_current_index(seg);
        self.state.channel_search.set_prompt_seg(seg, sel);
    }

    /// 把 prompt 焦点移回 query 段:光标落词首（从 kind chip 右移进 query 即落词头）。
    fn focus_query(&mut self) {
        self.state
            .channel_search
            .set_prompt_seg(PromptSegment::Query, 0);
        if let Some(session) = self.state.channel_search.current_mut() {
            session.cursor_home();
        }
    }

    /// chip 下拉高亮行移动(钳列表范围;空列表 no-op)。
    fn move_seg_sel(&mut self, seg: PromptSegment, down: bool) {
        let len = self.chip_options_len(seg);
        if len == 0 {
            return;
        }
        let cur = self.state.channel_search.seg_sel();
        let next = if down {
            cur.saturating_add(1).min(len - 1)
        } else {
            cur.saturating_sub(1)
        };
        self.state.channel_search.set_seg_sel(next);
    }

    /// chip 段 Enter:下拉展开时确认当前行（切 source / kind）并塌回;收起时重新展开。
    fn chip_seg_enter(&mut self, seg: PromptSegment) {
        if !self.state.channel_search.seg_open() {
            let sel = self.chip_current_index(seg);
            self.state.channel_search.open_seg(sel);
            return;
        }
        let sel = self.state.channel_search.seg_sel();
        match seg {
            PromptSegment::Source => self.confirm_source(sel),
            PromptSegment::Kind => self.confirm_kind(sel),
            PromptSegment::Query => {}
        }
        self.state.channel_search.close_seg();
    }

    /// 确认 source 选择:切到该 source（保留各 source 会话），首次进入 flash kind 提示;
    /// 焦点留在 source chip 段。
    fn confirm_source(&mut self, idx: usize) {
        let Some(source) = self
            .state
            .channel_search
            .source_options(&self.state.caps)
            .get(idx)
            .copied()
        else {
            return;
        };
        if let Some(kind) = self
            .state
            .channel_search
            .switch_source(source, &self.state.caps)
        {
            self.flash_kind_switched(kind);
        }
    }

    /// 确认 kind 选择:切到该 kind;无缓存 + query 非空则用当前 query 自动搜（焦点留在 kind chip）。
    fn confirm_kind(&mut self, idx: usize) {
        let Some(kind) = self
            .state
            .channel_search
            .kind_options(&self.state.caps)
            .get(idx)
            .copied()
        else {
            return;
        };
        if self.state.channel_search.select_kind(kind) {
            self.submit_current_query();
        }
    }

    /// chip 段 left:kind → source;source 已是最左,no-op。
    fn chip_seg_left(&mut self, seg: PromptSegment) {
        if seg == PromptSegment::Kind {
            self.focus_chip(PromptSegment::Source);
        }
    }

    /// chip 段 right:source → kind;kind → query 文本段。
    fn chip_seg_right(&mut self, seg: PromptSegment) {
        match seg {
            PromptSegment::Source => self.focus_chip(PromptSegment::Kind),
            PromptSegment::Kind => self.focus_query(),
            PromptSegment::Query => {}
        }
    }

    /// 某 chip 段当前选择在其列表里的下标（focus 到达时下拉高亮落此行）;找不到落 0。
    fn chip_current_index(&self, seg: PromptSegment) -> usize {
        match seg {
            PromptSegment::Source => {
                let cur = self.state.channel_search.source;
                self.state
                    .channel_search
                    .source_options(&self.state.caps)
                    .iter()
                    .position(|s| Some(*s) == cur)
                    .unwrap_or(0)
            }
            PromptSegment::Kind => {
                let cur = self.state.channel_search.current().map(|s| s.kind);
                self.state
                    .channel_search
                    .kind_options(&self.state.caps)
                    .iter()
                    .position(|k| Some(*k) == cur)
                    .unwrap_or(0)
            }
            PromptSegment::Query => 0,
        }
    }

    /// 某 chip 段下拉的候选数(走查钳制用)。
    fn chip_options_len(&self, seg: PromptSegment) -> usize {
        match seg {
            PromptSegment::Source => self
                .state
                .channel_search
                .source_options(&self.state.caps)
                .len(),
            PromptSegment::Kind => self
                .state
                .channel_search
                .kind_options(&self.state.caps)
                .len(),
            PromptSegment::Query => 0,
        }
    }

    /// 提交当前会话的首页搜索任务,焦点转结果列。空 query 不提交(留在 prompt)。
    /// 显式提交即作废旧词缓存(per-kind 桶按当前 query 重建)。
    fn submit_search(&mut self) {
        let Some(source) = self.state.channel_search.source else {
            return;
        };
        let (kind, query) = {
            let Some(session) = self.state.channel_search.current_mut() else {
                return;
            };
            if session.query().is_empty() {
                return;
            }
            let pair = (session.kind, session.query().to_owned());
            session.clear_results();
            pair
        };
        self.submit_query_task(source, kind, query);
        self.state.channel_search.set_focus(SearchFocus::Results);
    }

    /// 用当前会话 source/kind/query 提交一次搜索（不改焦点、不清其它 kind 桶）——
    /// 切 kind 自动搜用,结果落桶后焦点仍留在 chip 段。
    fn submit_current_query(&self) {
        let Some(source) = self.state.channel_search.source else {
            return;
        };
        let Some(session) = self.state.channel_search.current() else {
            return;
        };
        if session.query().is_empty() {
            return;
        }
        self.submit_query_task(source, session.kind, session.query().to_owned());
    }

    /// 提交一条首页 Search 任务(User 优先级)。
    fn submit_query_task(&self, source: SourceKind, kind: SearchKind, query: String) {
        self.client.submit_task(
            TaskKind::ChannelFetch(ChannelFetchKind::Search {
                source,
                kind,
                query,
                page: Page::default(),
            }),
            Priority::User,
        );
    }

    /// flash 提示「kind 已切到 xxx」(切 source 致 kind 落首项时)。
    fn flash_kind_switched(&mut self, kind: SearchKind) {
        self.notifications.flash(tinted_text_item(
            format!("kind \u{2192} {}", kind.label()),
            TextTint::Normal,
        ));
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
            f.list_sel = 2;
        }
        match app.detail_activate_action() {
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
        app.activate_detail_item();
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
            f.list_sel = 1;
        }
        // 走完整 handler:不只是返回动作,而要真发出 set_queue + play_song 两步。
        app.activate_detail_item();
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
        });
        app.state.channel_search.set_focus(SearchFocus::Results);
        if let Some(kr) = app.state.channel_search.active_results_mut() {
            kr.set_sel(2);
        }
        app.activate_search_panel();
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
        });
        app.state.channel_search.set_focus(SearchFocus::Results);
        app.activate_search_panel();
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
        });
        app.state.channel_search.set_focus(SearchFocus::Results);
        app.drill_search_panel();
        assert_eq!(
            app.state.channel_search.focus,
            SearchFocus::Detail,
            "drill 在结果列 → 进 detail(song 进其专辑)"
        );
        Ok(())
    }
}
