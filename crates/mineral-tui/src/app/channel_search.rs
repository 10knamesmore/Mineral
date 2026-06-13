//! Search 布局态的键盘输入执行器:token prompt 打字 / 结果列 / 详情面板,按 [`SearchFocus`] 分派。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mineral_channel_core::Page;
use mineral_task::{ChannelFetchKind, Priority, TaskKind};

use crate::runtime::action::{Action, SelectionMove};
use crate::runtime::keymap::chord_from_event;
use crate::runtime::state::SearchFocus;

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
        if matches!(key.code, KeyCode::Tab | KeyCode::Esc) {
            self.state.channel_search.set_focus(SearchFocus::Prompt);
            return;
        }
        match chord_from_event(key).and_then(|chord| self.keymap.lookup(chord)) {
            Some(Action::MoveSelection(mv)) => {
                if self.state.channel_search.focus == SearchFocus::Results {
                    self.move_search_result_sel(mv);
                }
            }
            Some(Action::ActivateSelection) => self.focus_search_panel_forward(),
            Some(Action::BackOrClearSearch) => self.focus_search_panel_backward(),
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

    /// 面板后退一格(detail → results;results 已是最左,无操作)。`back` 绑定触发。
    fn focus_search_panel_backward(&mut self) {
        if self.state.channel_search.focus == SearchFocus::Detail {
            self.state.channel_search.set_focus(SearchFocus::Results);
        }
    }

    /// 按一次 [`SelectionMove`] 移动当前会话结果列光标(钳首 / 末行)。
    fn move_search_result_sel(&mut self, mv: SelectionMove) {
        let Some(session) = self.state.channel_search.current_mut() else {
            return;
        };
        let last = session.result_len().saturating_sub(1);
        session.sel = match mv {
            SelectionMove::Down(n) => session.sel.saturating_add(n).min(last),
            SelectionMove::Up(n) => session.sel.saturating_sub(n),
            SelectionMove::First => 0,
            SelectionMove::Last => last,
        };
    }

    /// token prompt 输入:字符进 query、退格删末字,另含退出布局态 / 提交搜索 / 切面板三个控制动作。
    ///
    /// 带 CONTROL 的字符键吞掉(控制组合不污染 query)。
    fn handle_search_prompt_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Esc => self.state.channel_search.active.toggle(),
            KeyCode::Enter => self.submit_search(),
            KeyCode::Tab => {
                // 只切到上次面板位置,不提交搜索(区别于提交动作:搜索并切)。
                let target = self.state.channel_search.last_panel;
                self.state.channel_search.set_focus(target);
            }
            KeyCode::Char(_) if key.modifiers.contains(KeyModifiers::CONTROL) => {}
            KeyCode::Char(c) => {
                if let Some(session) = self.state.channel_search.current_mut() {
                    session.query.push(c);
                }
            }
            KeyCode::Backspace => {
                if let Some(session) = self.state.channel_search.current_mut() {
                    session.query.pop();
                }
            }
            _ => {}
        }
    }

    /// 提交当前会话的首页搜索任务,焦点转结果列。空 query 不提交(留在 prompt)。
    fn submit_search(&mut self) {
        let Some(source) = self.state.channel_search.source else {
            return;
        };
        let Some(session) = self.state.channel_search.current() else {
            return;
        };
        if session.query.is_empty() {
            return;
        }
        let kind = session.kind;
        let query = session.query.clone();
        self.client.submit_task(
            TaskKind::ChannelFetch(ChannelFetchKind::Search {
                source,
                kind,
                query,
                page: Page::default(),
            }),
            Priority::User,
        );
        self.state.channel_search.set_focus(SearchFocus::Results);
    }
}
