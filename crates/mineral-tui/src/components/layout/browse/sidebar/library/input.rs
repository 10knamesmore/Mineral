//! 本列表的输入、选择恢复和局部动画生命周期。

use super::TrackList;
use crate::components::layout::shared::filter_input;

impl TrackList {
    /// 当前光标和视口的只读位置。
    pub(crate) fn scroll(&self) -> &crate::runtime::scroll::list::ScrollList {
        &self.scroll
    }

    /// 当前查询与输入焦点。
    pub(crate) fn search(&self) -> &crate::runtime::state::SearchState {
        &self.search
    }

    /// 外部导航指定一个已有行；视口仍由布局准备更新。
    pub(crate) fn select(&mut self, index: usize) {
        self.scroll.set_sel(index);
    }

    /// 恢复离开前的屏幕位置。
    pub(crate) fn place(&mut self, index: usize, screen_row: usize) {
        self.scroll.place(index, screen_row);
    }

    /// 本地光标移动。
    pub(crate) fn move_selection(
        &mut self,
        movement: crate::runtime::action::SelectionMove,
        total: usize,
    ) {
        self.scroll.move_by(movement, total);
    }

    /// 同步移动视口和光标。
    pub(crate) fn scroll_by(&mut self, delta: i64, total: usize, ticks: u16) {
        self.scroll.page(delta, total, ticks);
    }

    /// 查询输入焦点进入或离开。
    pub(crate) fn set_typing(&mut self, typing: bool) {
        self.search.typing = typing;
    }

    /// 开启新的浏览上下文时清除旧过滤输入。
    pub(crate) fn reset_query(&mut self) {
        self.search.clear();
    }

    /// 根时钟推进本列表的展开动画。
    pub(crate) fn tick(&mut self) {
        self.expansion.tick();
    }

    /// 当前输入立即接管列表。
    pub(crate) fn interrupt(&mut self) {
        self.expansion.interrupt();
    }

    /// 数据或布局身份改变后放弃旧显示输入。
    pub(crate) fn invalidate(&mut self) {
        self.expansion.invalidate();
    }

    /// 配置重载时保留展开动画当前相位。
    pub(crate) fn retempo(&mut self, ticks: u16) {
        self.expansion.retempo(ticks);
    }

    /// 通过现有回归检查组件内部视口进度。
    #[cfg(test)]
    pub(crate) fn test_scroll_mut(&mut self) -> &mut crate::runtime::scroll::list::ScrollList {
        &mut self.scroll
    }

    /// 测试构造已有过滤输入，不重演按键。
    #[cfg(test)]
    pub(crate) fn test_search_mut(&mut self) -> &mut crate::runtime::state::SearchState {
        &mut self.search
    }

    /// 已准备展开输入的只读观察。
    #[cfg(test)]
    pub(crate) fn expansion(
        &self,
    ) -> &crate::runtime::state::ListExpansionState<mineral_model::CollectionIndex> {
        &self.expansion
    }
    /// 清除过滤并保留同一业务行及其屏幕位置。
    pub(crate) fn clear_filter(
        &mut self,
        input: super::TrackInput<'_>,
        config: &mineral_config::Config,
    ) {
        if self.search.query().is_empty() {
            return;
        }
        let selected = self
            .rows(input)
            .get(self.scroll.sel())
            .map(|entry| entry.data.index);
        self.search.clear();
        let entries = input
            .tracks
            .map_or(&[][..], |tracks| tracks.entries.as_slice());
        let index = selected.and_then(|selected| {
            entries
                .iter()
                .position(|entry| entry.data.index == selected)
        });
        filter_input::restore(
            &mut self.scroll,
            &mut self.expansion,
            index,
            entries
                .iter()
                .enumerate()
                .map(|(index, entry)| (entry.data.index, index)),
            config.tui().animation(),
        );
    }

    /// 编辑自己的过滤词，返回是否需要重启选择防抖。
    pub(crate) fn on_filter_key(
        &mut self,
        key: &crossterm::event::KeyEvent,
        input: super::TrackInput<'_>,
        config: &mineral_config::Config,
    ) -> bool {
        match filter_input::edit(&mut self.search, key) {
            filter_input::FilterEdit::Unchanged => false,
            filter_input::FilterEdit::Changed => {
                self.scroll.place(0, 0);
                true
            }
            filter_input::FilterEdit::Clear { typed } => {
                self.clear_filter(input, config);
                typed
            }
        }
    }
}
