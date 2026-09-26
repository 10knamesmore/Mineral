//! 搜索结果与详情面板的焦点导航、光标移动、下钻、分页和播放选择。

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};
use mineral_model::{Album, Song};
use mineral_task::SearchPayload;

use crate::app::page::Page;
use crate::runtime::action::{Action, ScrollStep, SelectionMove};
use crate::runtime::keymap::chord_from_event;
use crate::runtime::scroll::viewport::step_delta;
use crate::runtime::state::{ArtistSection, DetailData, EntityRef, SearchFocus, SearchPage};

use super::{SearchCtx, SearchEffect};

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
    /// 面板(results / detail)导航:搜索界面非文本输入,截获面板导航与当前行下载,其余键
    /// 回落全局 dispatch（[`SearchEffect::Dispatch`]）——transport(播放/音量/seek/模式)、
    /// 退出确认等照常生效。
    ///
    /// `activate` 前进、`back` 后退；移动键操作当前面板的列表，滚动键按配置步长移动
    /// 结果光标或详情简介。`EnterSearch` 回到 Query 输入段并保留搜索词。
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
            Some(Action::DownloadSelection) => self.download_selected_song(),
            Some(Action::Scroll(step)) => self.scroll_search_panel(step, ctx.behavior),
            Some(Action::EnterSearch) => {
                mineral_log::debug!(
                    target: "tui",
                    focus = ?self.focus,
                    "搜索面板返回查询输入"
                );
                self.set_focus(SearchFocus::Prompt);
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

    /// 把 Search 当前光标行转换成单曲下载意图；容器行没有对应下载语义时为空操作。
    fn download_selected_song(&self) -> SearchEffect {
        match self.selected_entity() {
            Some(EntityRef::Song(song)) => SearchEffect::Download(song),
            Some(EntityRef::Album(_) | EntityRef::Artist(_) | EntityRef::Playlist(_)) | None => {
                SearchEffect::None
            }
        }
    }

    /// 返回 Search 当前面板光标指向的实体；prompt 焦点、数据未到或光标越界时为 `None`。
    pub(crate) fn selected_entity(&self) -> Option<EntityRef> {
        match self.focus {
            SearchFocus::Results => self
                .active_results()
                .and_then(|results| EntityRef::from_payload(&results.results, results.sel())),
            SearchFocus::Detail => self
                .active_results()
                .and_then(|results| results.detail.current())
                .and_then(crate::runtime::state::DetailFrame::row_entity),
            SearchFocus::Prompt => None,
        }
    }

    /// 面板前进一格(results → detail;detail 已是最右,无操作)。`activate` 绑定触发。
    fn focus_search_panel_forward(&mut self) {
        if self.focus == SearchFocus::Results {
            self.set_focus(SearchFocus::Detail);
        }
    }

    /// 移动当前面板的列表光标；结果列与艺人专辑列表近底时预取各自的下一页。
    ///
    /// # Params:
    ///   - `mv`: 选择移动
    ///   - `prefetch_rows`: 列表预取触发半径(`behavior.search_prefetch_rows`)
    fn move_search_panel(&mut self, mv: SelectionMove, prefetch_rows: u16) -> SearchEffect {
        match self.focus {
            SearchFocus::Results => self.move_search_result_sel(mv, prefetch_rows),
            SearchFocus::Detail => self.move_detail_list_sel(mv, prefetch_rows),
            SearchFocus::Prompt => SearchEffect::None,
        }
    }

    /// 移动详情光标并钳在当前列表内；Albums 区近底时登记该艺人的待收取页。
    fn move_detail_list_sel(&mut self, mv: SelectionMove, prefetch_rows: u16) -> SearchEffect {
        self.last_sel_change = Instant::now();
        let Some(kr) = self.active_results_mut() else {
            return SearchEffect::None;
        };
        let Some(frame) = kr.detail.current_mut() else {
            return SearchEffect::None;
        };
        let len = frame.list_len();
        frame.list_mut().move_by(mv, len);
        let rows_to_bottom = len.saturating_sub(1).saturating_sub(frame.list().sel());
        if rows_to_bottom > usize::from(prefetch_rows) {
            return SearchEffect::None;
        }
        frame
            .request_artist_albums_page()
            .map_or(SearchEffect::None, |(id, page)| {
                SearchEffect::FetchArtistAlbums { id, page }
            })
    }

    /// 按配置步长移动结果光标或滚动详情简介。结果沿用列表移动的边界钳制、详情复位和
    /// 分页预取，视口由渲染端按 scrolloff 跟随；详情简介上界由渲染端按折行高度钳制。
    fn scroll_search_panel(
        &mut self,
        step: ScrollStep,
        behavior: &mineral_config::BehaviorConfig,
    ) -> SearchEffect {
        let delta = step_delta(step, behavior);
        mineral_log::debug!(
            target: "tui",
            focus = ?self.focus,
            delta,
            "滚动搜索面板"
        );
        match self.focus {
            SearchFocus::Results => {
                let rows = usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX);
                let movement = if delta >= 0 {
                    SelectionMove::Down(rows)
                } else {
                    SelectionMove::Up(rows)
                };
                self.move_search_result_sel(movement, *behavior.search_prefetch_rows())
            }
            SearchFocus::Detail => {
                if let Some(frame) = self.active_results().and_then(|kr| kr.detail.current()) {
                    frame.nudge_description(delta);
                }
                SearchEffect::None
            }
            SearchFocus::Prompt => SearchEffect::None,
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
            Some((queue, target)) => SearchEffect::PlayQueue {
                queue,
                target,
                context: self.search_context(),
            },
            None => {
                self.focus_search_panel_forward();
                SearchEffect::None
            }
        }
    }

    /// 顶层搜索结果起播的队列语境(埋点:Search{query},取 active session 查询词)。
    pub(crate) fn search_context(&self) -> mineral_protocol::QueueContextWire {
        let query = self
            .current()
            .map(|s| s.query().to_owned())
            .unwrap_or_default();
        mineral_protocol::QueueContextWire::Search { query }
    }

    /// detail 面板起播的队列语境:当前帧定出容器身份(专辑 / artist / 歌单)则报它,否则回落搜索词。
    pub(crate) fn detail_context(&self) -> mineral_protocol::QueueContextWire {
        self.active_results()
            .and_then(|kr| kr.detail.current())
            .and_then(crate::runtime::state::DetailFrame::play_context)
            .unwrap_or_else(|| self.search_context())
    }

    /// 结果列选中行若是 song,给出(整列队列, exact target);非 song 结果(容器)→ `None`。
    fn result_play_target(&self) -> Option<(Vec<Song>, usize)> {
        let kr = self.active_results()?;
        let SearchPayload::Songs(songs) = &kr.results else {
            return None;
        };
        let target = kr.sel();
        songs.get(target)?;
        Some((songs.clone(), target))
    }

    /// 面板下探(`drill_into`):results → 进 detail(song 进其专辑、容器进详情);
    /// detail → 下钻选中专辑(artist 专辑区;曲目是叶子,无可下钻)。纯状态,无副作用。
    fn drill_search_panel(&mut self, sweep_ticks: u16) {
        match self.focus {
            SearchFocus::Results => self.focus_search_panel_forward(),
            SearchFocus::Detail => self.drill_detail_item(sweep_ticks),
            SearchFocus::Prompt => {}
        }
    }

    /// detail 下探:只取「下钻专辑」那支(artist 专辑区选中专辑 push 帧),曲目是叶子无操作。
    /// 复用 [`Self::detail_activate_action`] 的判定,与 `activate` 同源——activate 接 Drill+Play、
    /// drill 只接 Drill。
    fn drill_detail_item(&mut self, sweep_ticks: u16) {
        if let DetailActivate::Drill(album) = self.detail_activate_action()
            && let Some(kr) = self.active_results_mut()
        {
            kr.detail.push(EntityRef::Album(album), sweep_ticks);
            self.last_sel_change = Instant::now();
        }
    }

    /// detail 激活:artist 专辑区选中 album → push 下钻帧(纯状态);其余列表选中 song → 替换队列播放。
    fn activate_detail_item(&mut self, sweep_ticks: u16) -> SearchEffect {
        match self.detail_activate_action() {
            DetailActivate::Drill(album) => {
                if let Some(kr) = self.active_results_mut() {
                    kr.detail.push(EntityRef::Album(album), sweep_ticks);
                    self.last_sel_change = Instant::now();
                }
                SearchEffect::None
            }
            DetailActivate::Play { queue, target } => SearchEffect::PlayQueue {
                queue,
                target,
                context: self.detail_context(),
            },
            DetailActivate::None => SearchEffect::None,
        }
    }

    /// 读当前 detail 帧 + 选中项,定出激活动作(纯读,不改状态)。
    fn detail_activate_action(&self) -> DetailActivate {
        let Some(frame) = self.active_results().and_then(|kr| kr.detail.current()) else {
            return DetailActivate::None;
        };
        match (&frame.entity, frame.section, &frame.data) {
            // artist 专辑区:选中专辑 → 下钻。
            (
                EntityRef::Artist(_),
                ArtistSection::Albums,
                Some(DetailData::Artist {
                    albums: Some(albs), ..
                }),
            ) => albs
                .items()
                .get(frame.list().sel())
                .map_or(DetailActivate::None, |a| {
                    DetailActivate::Drill(Box::new(a.clone()))
                }),
            // artist 热门曲:选中曲 → 播放。
            (
                EntityRef::Artist(_),
                ArtistSection::Hot,
                Some(DetailData::Artist {
                    detail: Some(a), ..
                }),
            ) => play_from(a.songs.clone(), frame.list().sel()),
            // 专辑详情(专辑帧 / 歌曲帧看所属专辑)曲目 → 播放。
            (_, _, Some(DetailData::Album(album))) => play_from(
                album
                    .tracks
                    .iter()
                    .map(|track| track.song.clone())
                    .collect(),
                frame.list().sel(),
            ),
            // 曲目列表(歌单帧)→ 播放。
            (_, _, Some(DetailData::PlaylistEntries(entries))) => play_from(
                entries.iter().map(|entry| entry.song.clone()).collect(),
                frame.list().sel(),
            ),
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
                if popped {
                    self.last_sel_change = Instant::now();
                } else {
                    self.set_focus(SearchFocus::Results);
                }
            }
            SearchFocus::Results => self.set_focus(SearchFocus::Prompt),
            SearchFocus::Prompt => {}
        }
    }

    /// 切 artist 双区(仅 artist 帧),光标归零并 arm 横向滑动。CycleDetailSection 经全局 keymap 派发、
    /// 任何焦点都可能到这里,仅 detail 焦点才动分区。
    fn cycle_detail_section(&mut self, sweep_ticks: u16) {
        if self.focus != SearchFocus::Detail {
            return;
        }
        self.last_sel_change = Instant::now();
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
        self.last_sel_change = Instant::now();
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
        let rows_to_bottom = last.saturating_sub(kr.sel());
        if rows_to_bottom > usize::from(prefetch_rows) {
            return SearchEffect::None;
        }
        self.fetch_more_effect()
    }

    /// 登记当前桶待收取的续页并生成提交意图；等待回包期间不重复请求。
    fn fetch_more_effect(&mut self) -> SearchEffect {
        let Some(source) = self.source else {
            return SearchEffect::None;
        };
        let Some(session) = self.current_mut() else {
            return SearchEffect::None;
        };
        if session.query().is_empty() {
            return SearchEffect::None;
        }
        let Some(results) = session.kind_results_mut() else {
            return SearchEffect::None;
        };
        let Some(page) = results.request_next_page() else {
            return SearchEffect::None;
        };
        SearchEffect::FetchMore {
            source,
            kind: session.kind,
            query: session.query().to_owned(),
            page,
        }
    }
}

/// detail 焦点 activate 的动作:下钻一张专辑、或替换队列播放选中曲。
enum DetailActivate {
    /// artist 专辑区选中专辑 → push 下钻帧。
    Drill(Box<Album>),

    /// 列表选中曲 → 替换队列播放(队列 = 整个列表,起播 = 选中曲)。
    Play {
        /// 替换进播放队列的曲目(整列表)。
        queue: Vec<Song>,

        /// `queue` 内的 0-based exact target coordinate。
        target: usize,
    },

    /// 无可激活项(列表空 / 数据未到)。
    None,
}

/// 从列表第 `sel` 首构造「替换队列播放」动作(队列 = 整个列表);越界为 None。
fn play_from(songs: Vec<Song>, target: usize) -> DetailActivate {
    if songs.get(target).is_none() {
        return DetailActivate::None;
    }
    DetailActivate::Play {
        queue: songs,
        target,
    }
}

#[cfg(test)]
mod tests {
    use mineral_channel_core::Page;
    use mineral_model::{AlbumId, SearchKind, SourceKind};
    use mineral_task::{SearchPayload, TaskEvent};

    use super::DetailActivate;
    use crate::runtime::state::SearchFocus;

    /// 专辑详情(`DetailData::Album`)里 activate 选中曲必须返回 `Play`(队列=专辑曲目、
    /// target=选中曲)；若落进 catch-all 返回 `None`,播放键会静默无效。
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
                    .tracks(mineral_model::AlbumTrack::enumerate(songs))
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
            DetailActivate::Play { queue, target } => {
                assert_eq!(queue.len(), 4, "队列=专辑全部曲目");
                assert_eq!(target, 2, "exact target=选中第 3 个 occurrence");
                assert_eq!(queue.get(target).map(|song| &song.id), want.as_ref());
            }
            DetailActivate::Drill(_) => color_eyre::eyre::bail!("专辑详情曲目不应下钻"),
            DetailActivate::None => {
                color_eyre::eyre::bail!("回归:专辑详情 activate 落进 catch-all、静默无反应")
            }
        }
        Ok(())
    }

    /// 造一个「专辑详情已在 detail 栈顶」的 probed App:结果列 1 张专辑、详情 4 首曲目。
    /// 高亮焦点系列测试共用。
    fn app_with_album_detail() -> color_eyre::Result<crate::App> {
        use mineral_model::{Album, AlbumId};

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
        app.state.apply(&TaskEvent::AlbumDetailFetched {
            id: AlbumId::new(SourceKind::NETEASE, "al1"),
            album: Box::new(
                Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                    .name("al1".to_owned())
                    .tracks(mineral_model::AlbumTrack::enumerate(
                        crate::test_support::endserenading(4),
                    ))
                    .build(),
            ),
        });
        Ok(app)
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
            has_more: None,
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
                    .tracks(mineral_model::AlbumTrack::enumerate(songs))
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
        // 走完整 handler:必须真发出 atomic PlayQueue。
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
        assert_eq!(
            *ops,
            vec![("play_queue", format!("3:1:{want_q}"))],
            "detail 应原子提交 queue + exact target"
        );
        Ok(())
    }

    /// detail 面板从专辑曲目起播时必须携带 Album 语境，不能沿用外层搜索词。
    #[test]
    fn detail_album_play_carries_album_context() -> color_eyre::Result<()> {
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
                    .tracks(mineral_model::AlbumTrack::enumerate(
                        crate::test_support::endserenading(3),
                    ))
                    .build(),
            ),
        });
        app.state.channel_search.set_focus(SearchFocus::Detail);
        let super::SearchEffect::PlayQueue { context, .. } = app
            .state
            .channel_search
            .activate_detail_item(/*sweep_ticks*/ 1)
        else {
            color_eyre::eyre::bail!("专辑详情 activate 应产出 PlayQueue");
        };
        assert_eq!(
            context,
            mineral_protocol::QueueContextWire::Album {
                id: AlbumId::new(SourceKind::NETEASE, "al1"),
                name: Some("al1".to_owned()),
            },
            "detail 专辑起播记 Album 语境(带页面标题快照)"
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
        assert_eq!(
            *ops,
            vec![("play_queue", format!("4:2:{want_q}"))],
            "song 结果应原子提交 queue + exact target 2"
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
                    .tracks(mineral_model::AlbumTrack::enumerate(
                        crate::test_support::endserenading(3),
                    ))
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

    /// 把面板选中时间戳拨回过去(越过防抖窗,checked_sub 防单调时钟下溢)。
    fn rewound() -> std::time::Instant {
        std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(3600))
            .unwrap_or_else(std::time::Instant::now)
    }

    /// search 面板导航必须刷新面板选中时间戳——封面滚动防抖与 detail 驻留预取窗都以它计时;
    /// 不刷新则 search 布局下两者恒失效(滚动即闪占位 / 每格光标立即打 API)。
    #[test]
    fn panel_nav_bumps_last_sel_change() -> color_eyre::Result<()> {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = app_with_album_detail()?;
        app.state.channel_search.set_focus(SearchFocus::Results);
        app.state.channel_search.last_sel_change = rewound();
        let before = app.state.channel_search.last_sel_change;
        app.handle_channel_search_key(&KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()));
        assert!(
            app.state.channel_search.last_sel_change > before,
            "结果列光标移动应刷新时间戳"
        );

        app.state.channel_search.set_focus(SearchFocus::Detail);
        app.state.channel_search.last_sel_change = rewound();
        let before = app.state.channel_search.last_sel_change;
        app.handle_channel_search_key(&KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()));
        assert!(
            app.state.channel_search.last_sel_change > before,
            "detail 列表光标移动应刷新时间戳"
        );
        Ok(())
    }

    /// 造一个「结果列 1 张带封面专辑、其详情帧在 detail 栈顶」的 probed App,返回封面 URL。
    /// 图片防抖系列测试共用；是否把图塞进解码缓存由各测试自定。
    fn app_with_covered_album() -> color_eyre::Result<(crate::App, mineral_model::MediaUrl)> {
        use mineral_model::{Album, AlbumId, MediaUrl};

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Album])?;
        if let Some(s) = app.state.channel_search.current_mut() {
            s.set_query("q");
        }
        let url = MediaUrl::remote("https://x.y/cover.jpg")?;
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Album,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Albums(vec![
                Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                    .name("al1".to_owned())
                    .cover_url(Some(url.clone()))
                    .build(),
            ]),
            has_more: None,
        });
        Ok((app, url))
    }

    /// 封面编码派发尊重 search 面板的滚动防抖：选中刚变时不投编码，停稳后只派发一次。
    #[test]
    fn search_cover_encode_respects_scroll_debounce() -> color_eyre::Result<()> {
        use std::sync::Arc;
        use std::time::Instant;

        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let (mut app, url) = app_with_covered_album()?;
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(64, 64));
        app.state.images.cache.insert_test(&url, Arc::new(img));

        let mut t = Terminal::new(TestBackend::new(120, 44))?;
        // 窗内(选中刚变):不投编码。
        app.state.channel_search.last_sel_change = Instant::now();
        t.draw(|f| crate::view::draw(f, &app))?;
        assert!(
            app.state.images.encode_pending.borrow().is_empty(),
            "滚动窗内不应派发封面编码"
        );
        // 停稳(窗外):恰好派发一次。
        app.state.channel_search.last_sel_change = rewound();
        t.draw(|f| crate::view::draw(f, &app))?;
        assert_eq!(
            app.state.images.encode_pending.borrow().len(),
            1,
            "停稳后应恰好派发一次封面编码"
        );
        Ok(())
    }
}
