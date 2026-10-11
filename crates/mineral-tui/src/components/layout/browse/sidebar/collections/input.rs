//! 本列表的输入、选择恢复和局部动画生命周期。

use crossterm::event::KeyEvent;

use super::CollectionList;
use super::window::CollectionWindow;
use crate::components::layout::shared::filter_input;
use crate::config::TuiConfig;
use crate::runtime::action::SelectionMove;
use crate::runtime::collection::{CollectionGroup, CollectionRef};
use crate::runtime::scroll::list::ScrollList;
use crate::runtime::state::{LibraryData, SearchState};
use ratatui::layout::Rect;

impl CollectionList {
    /// 菜单跟随抽屉压缩后的项目行，避开固定列名。
    pub(crate) fn selected_row_area(
        &self,
        area: Rect,
        library: &LibraryData,
        config: &TuiConfig,
    ) -> Rect {
        let rows = self.rows(library, config);
        let area = self.content_area(area);
        let window = CollectionWindow::new(area, &rows, &self.active().scroll);
        window
            .selected
            .and_then(|index| window.row_area(index))
            .unwrap_or(window.body)
    }

    /// 当前光标和视口的只读位置。
    pub(crate) fn scroll(&self) -> &ScrollList {
        &self.active().scroll
    }

    /// 数据刷新也保留未显示类型的光标。
    pub(crate) fn scroll_for(&self, group: CollectionGroup) -> &ScrollList {
        &self.state_for(group).scroll
    }

    /// 在指定类型的新排序中恢复选择，不切换当前页面。
    pub(crate) fn place_in(&mut self, group: CollectionGroup, index: usize, screen_row: usize) {
        self.state_for_mut(group).scroll.place(index, screen_row);
    }

    /// 当前查询与输入焦点。
    pub(crate) fn search(&self) -> &SearchState {
        &self.active().search
    }

    /// 外部导航指定一个已有行；视口仍由布局准备更新。
    #[cfg(test)]
    pub(crate) fn select(&mut self, index: usize) {
        self.active_mut().scroll.set_sel(index);
    }

    /// 恢复离开前的屏幕位置。
    pub(crate) fn place(&mut self, index: usize, screen_row: usize) {
        self.active_mut().scroll.place(index, screen_row);
    }

    /// 本地光标移动。
    pub(crate) fn move_selection(&mut self, movement: SelectionMove, total: usize) {
        self.active_mut().scroll.move_by(movement, total);
    }

    /// 同步移动视口和光标。
    pub(crate) fn scroll_by(&mut self, delta: i64, total: usize, ticks: u16) {
        self.active_mut().scroll.page(delta, total, ticks);
    }

    /// 查询输入焦点进入或离开。
    pub(crate) fn set_typing(&mut self, typing: bool) {
        self.active_mut().search.typing = typing;
    }

    /// 根时钟推进本列表的展开动画。
    pub(crate) fn tick(&mut self) {
        self.drawer.tick();
        self.playlists.expansion.tick();
        self.albums.expansion.tick();
    }

    /// 当前输入立即接管列表。
    pub(crate) fn interrupt(&mut self) {
        self.active_mut().expansion.interrupt();
    }

    /// 数据或布局身份改变后放弃旧显示输入。
    pub(crate) fn invalidate(&mut self) {
        self.playlists.expansion.invalidate();
        self.albums.expansion.invalidate();
    }

    /// 配置重载时保留展开动画当前相位。
    pub(crate) fn retempo(&mut self, ticks: u16, drawer_ticks: u16) {
        self.playlists.expansion.retempo(ticks);
        self.albums.expansion.retempo(ticks);
        self.drawer.retempo(drawer_ticks);
    }

    /// 测试构造已有过滤输入，不重演按键。
    #[cfg(test)]
    pub(crate) fn test_search_mut(&mut self) -> &mut SearchState {
        &mut self.active_mut().search
    }

    /// 清除过滤并保留同一业务行及其屏幕位置。
    pub(crate) fn clear_filter(&mut self, library: &LibraryData, config: &TuiConfig) {
        if self.active().search.query().is_empty() {
            return;
        }
        let selected = self
            .rows(library, config)
            .get(self.active().scroll.sel())
            .copied()
            .map(CollectionRef::id);
        self.active_mut().search.clear();
        let rows = self.rows(library, config);
        let index = selected.and_then(|id| rows.iter().position(|row| row.id() == id));
        let state = self.active_mut();
        filter_input::restore(
            &mut state.scroll,
            &mut state.expansion,
            index,
            rows.iter()
                .enumerate()
                .map(|(index, row)| (row.id(), index)),
            config.animation(),
        );
    }

    /// 编辑自己的过滤词，返回是否需要重启选择防抖。
    pub(crate) fn on_filter_key(
        &mut self,
        key: &KeyEvent,
        library: &LibraryData,
        config: &TuiConfig,
    ) -> bool {
        match filter_input::edit(&mut self.active_mut().search, key) {
            filter_input::FilterEdit::Unchanged => false,
            filter_input::FilterEdit::Changed => {
                self.active_mut().scroll.place(0, 0);
                true
            }
            filter_input::FilterEdit::Clear { typed } => {
                self.clear_filter(library, config);
                typed
            }
        }
    }
}
