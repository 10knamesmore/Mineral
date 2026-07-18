//! token prompt 的按键执行器:query 文本段打字 / 光标编辑,source·kind chip 段的下拉
//! 走查与确认,以及首页搜索提交。与结果面板导航分治,只在 prompt 焦点下被分派进来。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::runtime::state::{PromptSegment, SearchFocus, SearchPage};

use super::{SearchCtx, SearchEffect};

impl SearchPage {
    /// token prompt 按键:按当前段（query 文本 / source·kind chip）分派。
    /// 带 CONTROL 的字符键吞掉(控制组合不污染 query / 不误触段切换)。
    pub(super) fn handle_search_prompt_key(
        &mut self,
        key: &KeyEvent,
        ctx: SearchCtx<'_>,
    ) -> SearchEffect {
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

    /// 确认 source 选择:切到该 source（保留各 source 会话）;焦点留在 source chip 段。
    fn confirm_source(&mut self, idx: usize, ctx: SearchCtx<'_>) -> SearchEffect {
        let Some(source) = self.source_options(ctx.caps).get(idx).copied() else {
            return SearchEffect::None;
        };
        self.switch_source(source, ctx.caps);
        SearchEffect::None
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
