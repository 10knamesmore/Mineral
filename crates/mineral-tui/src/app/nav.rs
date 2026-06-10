//! 浏览态导航执行器:列表光标移动 / 视口滚动 / 视图进入与返回 / 搜索输入。
//!
//! 全部是 [`App::dispatch`](super::App) 查表命中后的执行端;全屏态的屏蔽闸在各执行器
//! 开头判,保证「键 → 行为」中段可被 config 表替换而闸语义不动。

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};
use mineral_model::Song;

use crate::runtime::action::{ScrollStep, SelectionMove};
use crate::runtime::scroll;
use crate::runtime::state::View;

use super::App;

impl App {
    /// 进入搜索输入态并清词;全屏态屏蔽(屏上无列表可滤)。
    pub(super) fn enter_search(&mut self) {
        if self.state.fullscreen {
            return;
        }
        self.state.search_mode = true;
        self.state.search_q.clear();
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
                if let Some(target_id) = self
                    .state
                    .filtered_playlists()
                    .get(self.state.sel_playlist)
                    .map(|p| p.data.id.clone())
                {
                    self.state.search_q.clear();
                    if let Some(raw_idx) = self
                        .state
                        .playlists
                        .iter()
                        .position(|p| p.data.id == target_id)
                    {
                        self.state.sel_playlist = raw_idx;
                    }
                }
                self.state.view = View::Library;
                self.state.view_pos.enter();
                self.state.sel_track = 0;
                // 新进一张歌单从头看:视口直接落位,不从上张歌单的深处滑回来。
                self.state.scroll_track.snap_to(0);
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
            self.state.view = View::Playlists;
            self.state.view_pos.leave();
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
