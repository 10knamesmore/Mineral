//! 浏览态导航执行器:列表光标移动 / 视口滚动 / 视图进入与返回 / 搜索输入。
//!
//! 全部是 [`App::dispatch`](super::App) 查表命中后的执行端;全屏态的屏蔽闸在各执行器
//! 开头判,保证「键 → 行为」中段可被 config 表替换而闸语义不动。

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mineral_model::{PlaylistId, Song};
use mineral_task::{ChannelFetchKind, Priority, TaskKind};

use crate::runtime::action::{ScrollStep, SelectionMove};
use crate::runtime::scroll;
use crate::runtime::state::View;
use crate::runtime::track_pos::{PendingRestore, TrackPos};

use super::App;

impl App {
    /// 进入搜索输入态并清词;全屏态屏蔽(屏上无列表可滤)。
    pub(super) fn enter_search(&mut self) {
        if self.state.fullscreen {
            return;
        }
        self.state.search_mode = true;
        self.state.search_q.clear();
        self.request_deep_search_tracks();
    }

    /// 深度搜索的数据保障:Playlists 视图进搜索态时,把所有未拉取的歌单曲目一次性
    /// 以 Background 优先级补齐(不抢视口 prefetch 的 User 档)。结果渐进到达,
    /// 过滤结果逐帧变全。`tracks_requested` 成败都记,失败歌单不会反复重提交。
    fn request_deep_search_tracks(&mut self) {
        if self.state.view != View::Playlists || !*self.state.cfg.tui().search().deep() {
            return;
        }
        let pending: Vec<PlaylistId> = self
            .state
            .playlists
            .iter()
            .map(|p| &p.data.id)
            .filter(|id| {
                !self.state.tracks_cache.contains_key(*id)
                    && !self.state.tracks_requested.contains(*id)
            })
            .cloned()
            .collect();
        for id in pending {
            self.client.submit_task(
                TaskKind::ChannelFetch(ChannelFetchKind::PlaylistTracks { id: id.clone() }),
                Priority::Background,
            );
            self.state.tracks_requested.insert(id);
        }
    }

    /// 搜索词每次变化后,把当前 view 的 sel 拉回 0(视口同步落位,逐字输入不滑屏)。
    fn reset_sel_for_search(&mut self) {
        match self.state.view {
            View::Playlists => {
                self.state.sel_playlist = 0;
                self.state.scroll_playlist.snap_to(0);
            }
            View::Library => {
                self.state.sel_track = 0;
                self.state.scroll_track.snap_to(0);
            }
        }
    }

    /// 搜索输入态按键:Esc 退出 + 清词,Enter 退出保留词,Backspace 删字符 / 空词上退出(vim
    /// 命令行行为),字符追加词;改词后复位 sel。
    ///
    /// 带 CONTROL 的字符键一律吞掉不当输入——否则 `<C-d>` 族滚动键在搜索态会把
    /// 裸字符塞进 query。
    pub(super) fn handle_search_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.state.search_mode = false;
                self.state.search_q.clear();
            }
            KeyCode::Enter => {
                self.state.search_mode = false;
            }
            KeyCode::Backspace => {
                // vim 行为:query 已空时再删一次 = 退出搜索(等价 Esc)。
                if self.state.search_q.is_empty() {
                    self.state.search_mode = false;
                    return;
                }
                self.state.search_q.pop();
                self.reset_sel_for_search();
                self.state.last_sel_change = Instant::now();
            }
            KeyCode::Char(_) if key.modifiers.contains(KeyModifiers::CONTROL) => {}
            KeyCode::Char(c) => {
                self.state.search_q.push(c);
                self.reset_sel_for_search();
                self.state.last_sel_change = Instant::now();
            }
            _ => {}
        }
    }

    /// 列表光标移动,按当前 view 落到 `sel_playlist` / `sel_track`,越界钳首末行;
    /// 全屏态屏蔽(屏上无列表)。
    pub(super) fn move_selection(&mut self, mv: SelectionMove) {
        if self.state.fullscreen {
            return;
        }
        self.state.last_sel_change = Instant::now();
        let max = match self.state.view {
            View::Playlists => self.state.filtered_playlists().len().saturating_sub(1),
            View::Library => self.state.filtered_tracks().len().saturating_sub(1),
        };
        let sel = match self.state.view {
            View::Playlists => &mut self.state.sel_playlist,
            View::Library => &mut self.state.sel_track,
        };
        *sel = match mv {
            SelectionMove::Down(n) => sel.saturating_add(n).min(max),
            SelectionMove::Up(n) => sel.saturating_sub(n),
            SelectionMove::First => 0,
            SelectionMove::Last => max,
        };
    }

    /// `<C-d>` 族滚动按上下文路由:全屏滚歌词;浏览态滚当前列表——视口目标与光标同移
    /// n 行(vim `<C-d>` 语义,保持光标的屏上相对位置),文档首尾边界由渲染端统一钳。
    pub(super) fn scroll(&mut self, step: ScrollStep) {
        if self.state.fullscreen {
            self.state.scroll_lyrics(step);
            return;
        }
        let delta = scroll::step_delta(step, self.state.cfg.tui().behavior());
        let rows = usize::try_from(delta.unsigned_abs()).unwrap_or(usize::MAX);
        let ticks = self.state.list_glide_ticks();
        let list = match self.state.view {
            View::Playlists => &self.state.scroll_playlist,
            View::Library => &self.state.scroll_track,
        };
        list.nudge(delta, ticks);
        self.move_selection(if delta > 0 {
            SelectionMove::Down(rows)
        } else {
            SelectionMove::Up(rows)
        });
    }

    /// 在当前视图「进入」:Playlists 进选中歌单的 Library;Library 设 queue 并播放选中曲。
    /// 全屏态屏蔽。
    pub(super) fn activate_selection(&mut self) {
        if self.state.fullscreen {
            return;
        }
        self.state.last_sel_change = Instant::now();
        match self.state.view {
            View::Playlists => {
                // 上一次进歌单挂的延迟恢复(曲目未到就退出来了)就此作废。
                self.state.pending_track_restore = None;
                let mut sel_track = 0usize;
                // 记忆恢复时的屏上相对行;None = 默认落位(光标上方留 scrolloff)。
                let mut screen_anchor: Option<usize> = None;
                if let Some(target_id) = self
                    .state
                    .filtered_playlists()
                    .get(self.state.sel_playlist)
                    .map(|p| p.data.id.clone())
                {
                    // 深度命中行:进歌单后光标直接落到命中歌(搜索 → 定位闭环,
                    // `search.locate_on_enter` 可关)。必须在清词前取——
                    // deep_hit_for 对空 query 恒 None。
                    let locate = (*self.state.cfg.tui().search().locate_on_enter())
                        .then(|| self.state.deep_hit_for(&target_id).map(|h| h.song_id))
                        .flatten();
                    self.state.search_q.clear();
                    if let Some(raw_idx) = self
                        .state
                        .playlists
                        .iter()
                        .position(|p| p.data.id == target_id)
                    {
                        self.state.sel_playlist = raw_idx;
                    }
                    if let Some(idx) = locate.and_then(|song_id| {
                        self.state
                            .tracks_cache
                            .get(&target_id)
                            .and_then(|ts| ts.iter().position(|sv| sv.data.id == song_id))
                    }) {
                        sel_track = idx;
                    } else if self.state.track_memory().enabled()
                        && let Some(pos) = self.state.track_pos.get(&target_id).cloned()
                    {
                        // 记忆恢复:深度命中优先(显式搜索意图压过历史位置),
                        // 走到这里说明无命中。曲目还没拉到时挂 pending,
                        // 等 `PlaylistTracksFetched` 补落位。
                        if let Some(tracks) = self.state.tracks_cache.get(&target_id) {
                            sel_track = pos.resolve(tracks);
                            // 恢复屏上相对位置:该行回到离开时的视口行,
                            // 而非统一顶到 scrolloff 位。
                            screen_anchor = Some(pos.screen_row);
                        } else {
                            self.state.pending_track_restore = Some(PendingRestore {
                                playlist: target_id.clone(),
                                pos,
                            });
                        }
                    }
                }
                self.state.view = View::Library;
                self.state.view_pos.enter();
                self.state.sel_track = sel_track;
                // 视口直接落位(记忆按屏上相对行还原;命中歌上方留 scrolloff;
                // 无命中即从头看),不从上张歌单的深处滑回来。
                let anchor = screen_anchor.unwrap_or_else(|| self.state.scrolloff());
                self.state
                    .scroll_track
                    .snap_to(sel_track.saturating_sub(anchor));
            }
            View::Library => {
                let filtered = self.state.filtered_tracks();
                let Some(song) = filtered.get(self.state.sel_track).map(|sv| sv.data.clone())
                else {
                    return;
                };
                let new_queue: Vec<Song> = self
                    .state
                    .current_tracks()
                    .into_iter()
                    .map(|sv| sv.data)
                    .collect();
                // Server 端按 PlayMode 决定要不要洗牌;client 只发原始 queue + target。
                self.client.set_queue(new_queue, song.id.clone());
                self.client.play_song(song);
            }
        }
    }

    /// 在当前视图「返回」:搜索非空先清词(选中复位),否则 Library 回 Playlists、
    /// Playlists 无处可回即无操作。全屏态屏蔽。
    pub(super) fn back_or_clear_search(&mut self) {
        if self.state.fullscreen {
            return;
        }
        self.state.last_sel_change = Instant::now();
        if !self.state.search_q.is_empty() {
            self.state.search_q.clear();
            self.reset_sel_for_search();
            return;
        }
        if matches!(self.state.view, View::Library) {
            self.remember_track_pos();
            self.state.pending_track_restore = None;
            self.state.view = View::Playlists;
            self.state.view_pos.leave();
        }
    }

    /// 把 Library 当前光标记入位置记忆表(`behavior.remember_track_pos` 非 off);
    /// persist 档随手把整表 fire-and-forget 落盘。
    ///
    /// 曲目未就绪 / 空歌单时不记,**保留旧记忆**——进了还没加载完的歌单就退出来,
    /// 不该把上次的有效位置抹成空。搜索过滤态下 `sel_track` 指向 filtered 列表,
    /// 记忆统一锚定到 raw 下标(恢复时无过滤)。
    pub(super) fn remember_track_pos(&mut self) {
        if !self.state.track_memory().enabled() || self.state.view != View::Library {
            return;
        }
        let Some(pid) = self.state.selected_playlist().map(|p| p.data.id.clone()) else {
            return;
        };
        let Some(song_id) = self
            .state
            .filtered_tracks()
            .get(self.state.sel_track)
            .map(|sv| sv.data.id.clone())
        else {
            return;
        };
        let index = self
            .state
            .current_tracks()
            .iter()
            .position(|sv| sv.data.id == song_id)
            .unwrap_or(self.state.sel_track);
        // 屏上相对行:光标减当前滚动目标(渲染端维护的视口首行)。
        let screen_row = self
            .state
            .sel_track
            .saturating_sub(self.state.scroll_track.target_rows());
        self.state.track_pos.insert(
            pid,
            TrackPos {
                song_id,
                index,
                screen_row,
            },
        );
        if self.state.track_memory().persists() {
            self.ui_prefs.save_track_positions(&self.state.track_pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use mineral_model::SourceKind;

    use super::super::App;
    use crate::test_support::app_with_library;

    /// 喂一个 Press 键给 App(走真实事件入口 `handle_event`)。
    fn press(app: &mut App, code: KeyCode) {
        app.handle_event(&Event::Key(KeyEvent::new(code, KeyModifiers::empty())));
    }

    /// 列表导航逐键回归:j/k/J/K/g/G 在 Library 与 Playlists 两个视图移动选中,
    /// 大跨步长 7、越界钳到首末行。
    #[test]
    fn j_k_navigate_in_playlists_and_library() -> color_eyre::Result<()> {
        // Library:10 首,从 0 起。
        let mut app = app_with_library(10, /*sel_track*/ 0)?;
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.state.sel_track, 1, "j 下移一行");
        press(&mut app, KeyCode::Char('J'));
        assert_eq!(app.state.sel_track, 8, "J 大跨下移 7 行");
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.state.sel_track, 9, "下移越界钳到末行");
        press(&mut app, KeyCode::Char('K'));
        assert_eq!(app.state.sel_track, 2, "K 大跨上移 7 行");
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.state.sel_track, 1, "k 上移一行");
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.state.sel_track, 0, "g 跳首行");
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.state.sel_track, 9, "G 跳末行");

        // Playlists:3 张歌单,同一组键作用于 sel_playlist。
        let mut app = app_with_library(3, /*sel_track*/ 0)?;
        app.state.view = crate::runtime::state::View::Playlists;
        app.state.playlists = vec![
            crate::test_support::playlist_view("p1", "A", SourceKind::NETEASE, 1),
            crate::test_support::playlist_view("p2", "B", SourceKind::NETEASE, 1),
            crate::test_support::playlist_view("p3", "C", SourceKind::NETEASE, 1),
        ];
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.state.sel_playlist, 1, "j 下移一行");
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.state.sel_playlist, 2, "G 跳末行");
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.state.sel_playlist, 1, "k 上移一行");
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.state.sel_playlist, 0, "g 跳首行");
        Ok(())
    }

    /// Playlists 视图 `l` 进 Library;Library 视图 Enter 触发 set_queue+play
    /// (TestClient no-op,断 view 流转与不 panic)。
    #[test]
    fn l_enters_library_enter_plays() -> color_eyre::Result<()> {
        let mut app = app_with_library(3, /*sel_track*/ 2)?;
        app.state.view = crate::runtime::state::View::Playlists;
        app.state.sel_playlist = 0;

        press(&mut app, KeyCode::Char('l'));
        assert_eq!(
            app.state.view,
            crate::runtime::state::View::Library,
            "l 应进 Library"
        );
        assert_eq!(app.state.sel_track, 0, "进 Library 选中复位到首行");

        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.state.view,
            crate::runtime::state::View::Library,
            "Enter 播放不切视图"
        );
        Ok(())
    }

    /// 搜索退格:逐字符删、删到空仍在搜索态,空 query 上再退格 = 退出搜索(vim 行为)。
    #[test]
    fn search_backspace_on_empty_query_exits() -> color_eyre::Result<()> {
        let mut app = app_with_library(3, /*sel_track*/ 0)?;

        // `/` 进入搜索输入态,query 起始为空。
        press(&mut app, KeyCode::Char('/'));
        assert!(app.state.search_mode, "`/` 应进入搜索态");
        assert!(app.state.search_q.is_empty());

        // 输入两个字符。
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(app.state.search_q, "ab");

        // 退格逐字符删;删到空时仍停在搜索态(不提前退出)。
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Backspace);
        assert!(app.state.search_q.is_empty());
        assert!(app.state.search_mode, "删到空时仍应在搜索态");

        // 空 query 上再删一次 → 退出搜索。
        press(&mut app, KeyCode::Backspace);
        assert!(!app.state.search_mode, "空 query 上退格应退出搜索");
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
        assert_eq!(app.state.sel_track, page, "C-f 翻页下移 page_scroll_rows");
        ctrl(&mut app, 'd');
        assert_eq!(
            app.state.sel_track,
            page + line,
            "C-d 单行档下移 line_scroll_rows"
        );
        ctrl(&mut app, 'u');
        ctrl(&mut app, 'b');
        assert_eq!(app.state.sel_track, 0, "C-u/C-b 对称滚回顶");
        ctrl(&mut app, 'b');
        assert_eq!(app.state.sel_track, 0, "顶部再上滚钳住不越界");

        app.state.fullscreen = true;
        ctrl(&mut app, 'f');
        assert_eq!(app.state.sel_track, 0, "全屏态 C-f 路由去歌词,列表光标不动");
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
        assert_eq!(app.state.search_q, "ab", "CONTROL 组合不进 query");
        assert!(app.state.search_mode, "也不退出搜索态");
        Ok(())
    }

    /// 深度命中定位:Playlists 视图搜到歌单内的歌后 Enter 进歌单,光标直接落在
    /// 命中歌上(而非第 0 行)。
    #[test]
    fn enter_on_deep_hit_locates_song_in_library() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;

        use crate::runtime::view_model::SongView;
        use crate::test_support::{song, with_name};

        let (mut app, _submitted) = crate::test_support::app_with_playlists_probed()?;
        let pid = PlaylistId::new(SourceKind::NETEASE, "p2");
        let views = ["甲", "乙", "春日影"]
            .into_iter()
            .map(|n| SongView {
                data: with_name(song(n), n),
                loved: false,
                plays: None,
            })
            .collect::<Vec<SongView>>();
        app.state.tracks_cache.insert(pid, views);
        app.state.tracks_generation = 1;

        press(&mut app, KeyCode::Char('/'));
        for c in "春日影".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter); // 提交过滤,保留词
        press(&mut app, KeyCode::Enter); // activate 选中歌单
        assert_eq!(
            app.state.view,
            crate::runtime::state::View::Library,
            "Enter 应进 Library"
        );
        assert_eq!(app.state.sel_track, 2, "光标应落在命中歌「春日影」上");
        assert!(app.state.search_q.is_empty(), "进歌单后清词(现状语义)");
        Ok(())
    }

    /// 配置旋钮:`search.locate_on_enter = false` 时,深度命中行 Enter 进歌单
    /// 仍从第 0 行开始(不定位)。
    #[test]
    fn locate_on_enter_disabled_keeps_top() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;

        use crate::runtime::view_model::SongView;
        use crate::test_support::{song, with_name};

        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.lua");
        std::fs::write(
            &path,
            "return { tui = { search = { locate_on_enter = false } } }",
        )?;
        let (mut app, _submitted) = crate::test_support::app_with_playlists_probed()?;
        app.reload_config_from(&path);
        let pid = PlaylistId::new(SourceKind::NETEASE, "p2");
        let views = ["甲", "乙", "春日影"]
            .into_iter()
            .map(|n| SongView {
                data: with_name(song(n), n),
                loved: false,
                plays: None,
            })
            .collect::<Vec<SongView>>();
        app.state.tracks_cache.insert(pid, views);
        app.state.tracks_generation = 1;

        press(&mut app, KeyCode::Char('/'));
        for c in "春日影".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.state.sel_track, 0, "旋钮关闭:不定位,从头看");
        Ok(())
    }

    /// 深度搜索数据保障:Playlists 视图按 `/`,所有未缓存歌单的 PlaylistTracks 一次性
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
                        TaskKind::ChannelFetch(ChannelFetchKind::PlaylistTracks { .. })
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
        app.state.view = crate::runtime::state::View::Library;
        app.state.tracks_requested.clear();
        press(&mut app, KeyCode::Char('/'));
        let tasks = submitted
            .lock()
            .map_err(|e| color_eyre::eyre::eyre!("探针锁中毒: {e}"))?;
        assert_eq!(tasks.len(), 3, "Library 视图进搜索态不应提交补拉");
        Ok(())
    }

    /// 配置总开关:`search.deep = false` 时按 `/` 不触发任何补拉。
    #[test]
    fn slash_respects_deep_disabled() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.lua");
        std::fs::write(&path, "return { tui = { search = { deep = false } } }")?;
        let (mut app, submitted) = crate::test_support::app_with_playlists_probed()?;
        app.reload_config_from(&path);
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
        assert_eq!(app.state.sel_track, 3, "前置:光标已下移");

        press(&mut app, KeyCode::Char('h'));
        assert_eq!(
            app.state.view,
            crate::runtime::state::View::Playlists,
            "h 退回 Playlists"
        );
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.state.sel_track, 3, "再进同一歌单应恢复原位");
        Ok(())
    }

    /// 相对位置记忆(端到端,经真实渲染):离开时光标在屏上第 N 行,再进恢复后
    /// 仍在第 N 行——而非统一顶到 scrolloff 位。
    #[test]
    fn reenter_restores_screen_relative_position() -> color_eyre::Result<()> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = crate::test_support::app_with_long_library(100, /*sel_track*/ 0)?;
        // fixture 只设了 view 标志;sidebar 按 view_pos 端点选画哪个视图,推到 Library 端。
        app.state.view_pos.enter();
        for _ in 0..40 {
            app.state.view_pos.tick();
        }
        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        app.state.sel_track = 50;
        // 渲染若干帧让视口收敛(光标深处 → offset > 0,光标落在视口下安全边界)。
        for _ in 0..40 {
            t.draw(|f| crate::view::draw(f, &app))?;
        }
        let off = app.state.scroll_track.target_rows();
        assert!(off > 0 && off <= 50, "前置:视口已滚到深处: {off}");
        let row_before = 50_usize.saturating_sub(off);

        press(&mut app, KeyCode::Char('h'));
        press(&mut app, KeyCode::Char('l'));
        for _ in 0..5 {
            t.draw(|f| crate::view::draw(f, &app))?;
        }
        assert_eq!(app.state.sel_track, 50, "光标恢复原行");
        let row_after = 50_usize.saturating_sub(app.state.scroll_track.target_rows());
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
        app.reload_config_from(&path);
        for _ in 0..3 {
            press(&mut app, KeyCode::Char('j'));
        }
        press(&mut app, KeyCode::Char('h'));
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.state.sel_track, 0, "off 档不记不恢复");
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
        if let Some(tracks) = app.state.tracks_cache.get_mut(&pid)
            && !tracks.is_empty()
        {
            tracks.remove(0);
        }
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.state.sel_track, 4, "同一首歌删行后顺移到 4");
        Ok(())
    }

    /// 优先级:深度搜索命中定位压过记忆位置——记忆说在第 0 行,命中歌在第 2 行,
    /// 进歌单应落在命中歌上。
    #[test]
    fn deep_hit_beats_remembered_position() -> color_eyre::Result<()> {
        use mineral_model::PlaylistId;

        use crate::runtime::track_pos::TrackPos;
        use crate::runtime::view_model::SongView;
        use crate::test_support::{song, with_name};

        let (mut app, _submitted) = crate::test_support::app_with_playlists_probed()?;
        let pid = PlaylistId::new(SourceKind::NETEASE, "p2");
        let views = ["甲", "乙", "春日影"]
            .into_iter()
            .map(|n| SongView {
                data: with_name(song(n), n),
                loved: false,
                plays: None,
            })
            .collect::<Vec<SongView>>();
        app.state.tracks_cache.insert(pid.clone(), views);
        app.state.tracks_generation = 1;
        app.state.track_pos.insert(
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
        assert_eq!(app.state.sel_track, 2, "深度命中应压过记忆位置");
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
        app.state.track_pos.insert(
            pid.clone(),
            TrackPos {
                song_id: song("丙").id,
                index: 2,
                screen_row: 0,
            },
        );
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.state.sel_track, 0, "曲目未到先停第 0 行");
        assert!(
            app.state
                .pending_track_restore
                .as_ref()
                .is_some_and(|p| p.playlist == pid),
            "应挂起该歌单的 pending 恢复"
        );

        // 退出再进别处之前,pending 不应泄漏到后续进入。
        press(&mut app, KeyCode::Char('h'));
        assert!(
            app.state.pending_track_restore.is_none(),
            "退出 Library 应作废 pending"
        );
        Ok(())
    }

    /// 全屏态屏蔽列表导航 + 搜索 `/`;
    #[test]
    fn fullscreen_blocks_nav_and_search() -> color_eyre::Result<()> {
        let mut app = app_with_library(5, /*sel_track*/ 2)?;
        press(&mut app, KeyCode::Char('z'));
        assert!(app.state.fullscreen);

        // j / g 导航被吞,选中不变。
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.state.sel_track, 2, "全屏屏蔽列表导航");

        // `/` 不进搜索态。
        press(&mut app, KeyCode::Char('/'));
        assert!(!app.state.search_mode, "全屏屏蔽搜索 `/`");
        Ok(())
    }
}
