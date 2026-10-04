//! 本列表的输入、选择恢复和局部动画生命周期。

use super::PlaylistList;
use crate::components::layout::shared::filter_input;

impl PlaylistList {
    /// 当前光标和视口的只读位置。
    pub(crate) fn scroll(&self) -> &crate::runtime::scroll::list::ScrollList {
        &self.scroll
    }

    /// 当前查询与输入焦点。
    pub(crate) fn search(&self) -> &crate::runtime::state::SearchState {
        &self.search
    }

    /// 外部导航指定一个已有行；视口仍由布局准备更新。
    #[cfg(test)]
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

    /// 测试构造已有过滤输入，不重演按键。
    #[cfg(test)]
    pub(crate) fn test_search_mut(&mut self) -> &mut crate::runtime::state::SearchState {
        &mut self.search
    }

    /// 清除过滤并保留同一业务行及其屏幕位置。
    pub(crate) fn clear_filter(
        &mut self,
        library: &crate::runtime::state::LibraryData,
        config: &mineral_config::Config,
    ) {
        if self.search.query().is_empty() {
            return;
        }
        let selected = self
            .rows(library, config)
            .get(self.scroll.sel())
            .map(|playlist| playlist.data.id.clone());
        self.search.clear();
        let index = selected.and_then(|selected| {
            library
                .playlists
                .iter()
                .position(|playlist| playlist.data.id == selected)
        });
        filter_input::restore(
            &mut self.scroll,
            &mut self.expansion,
            index,
            library
                .playlists
                .iter()
                .enumerate()
                .map(|(index, playlist)| (playlist.data.id.clone(), index)),
            config.tui().animation(),
        );
    }

    /// 编辑自己的过滤词，返回是否需要重启选择防抖。
    pub(crate) fn on_filter_key(
        &mut self,
        key: &crossterm::event::KeyEvent,
        library: &crate::runtime::state::LibraryData,
        config: &mineral_config::Config,
    ) -> bool {
        match filter_input::edit(&mut self.search, key) {
            filter_input::FilterEdit::Unchanged => false,
            filter_input::FilterEdit::Changed => {
                self.scroll.place(0, 0);
                true
            }
            filter_input::FilterEdit::Clear { typed } => {
                self.clear_filter(library, config);
                typed
            }
        }
    }
}
