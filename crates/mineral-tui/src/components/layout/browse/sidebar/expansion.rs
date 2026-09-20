//! 浏览页稳定列表接入共用的清除筛选展开；视图扫入与全屏形变不参与。

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::components::layout::shared::list_expansion::{self, ListSurface};
use crate::render::theme::Theme;
use crate::runtime::state::{AppState, ListExpansionScope, ListRowIdentity, View};

/// 在列表盖住氛围背景前准备展开画面。
pub(super) fn begin_list(buf: &Buffer, area: Rect, state: &AppState, view: View) -> ListSurface {
    let body = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(2),
        area.width.saturating_sub(2),
        area.height.saturating_sub(3),
    );
    list_expansion::begin_list(
        buf,
        area,
        body,
        &state.browse.list_expansion.borrow(),
        ListExpansionScope::Browse(view),
    )
}

/// 保留已画出的搜索结果，清除后在原有位置逐行展开。
pub(super) fn finish_list(
    buf: &mut Buffer,
    state: &AppState,
    theme: &Theme,
    surface: ListSurface,
    visible: Range<usize>,
    identities: impl Iterator<Item = ListRowIdentity>,
) {
    if surface.scope != ListExpansionScope::Browse(state.browse.view.current())
        || !state.browse.fullscreen.at_min()
        || !state.channel_search.active.at_min()
        || !(state.browse.view.at_min() || state.browse.view.at_max())
    {
        return;
    }
    list_expansion::finish_list(
        buf,
        &mut state.browse.list_expansion.borrow_mut(),
        theme,
        surface,
        visible,
        identities,
        !state.browse.active_search().query().is_empty(),
    );
}
