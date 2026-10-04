//! 浏览页稳定列表接入共用的清除筛选展开；视图扫入与全屏形变不参与。

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::components::layout::shared::list_expansion::{self, ListSurface};
use crate::render::theme::Theme;
use crate::runtime::state::{AppState, ListExpansionScope, View};

/// 在列表盖住氛围背景前准备展开画面。
pub(super) fn begin_list(buf: &Buffer, area: Rect, state: &AppState, view: View) -> ListSurface {
    let body = body(area);
    list_expansion::begin_list(
        buf,
        area,
        body,
        &state.browse.list_expansion,
        ListExpansionScope::Browse(view),
    )
}

/// 用已准备的搜索结果，在清除后按原有位置逐行展开。
pub(super) fn finish_list(
    buf: &mut Buffer,
    state: &AppState,
    theme: &Theme,
    surface: ListSurface,
    visible: Range<usize>,
) {
    if surface.scope != ListExpansionScope::Browse(state.browse.view.current())
        || !state.browse.fullscreen.at_min()
        || !state.channel_search.active.at_min()
        || !(state.browse.view.at_min() || state.browse.view.at_max())
    {
        return;
    }
    list_expansion::finish_list(buf, &state.browse.list_expansion, theme, surface, visible);
}

/// 数据行区域，准备与绘制共用。
pub(super) fn body(area: Rect) -> Rect {
    let body = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(2),
        area.width.saturating_sub(2),
        area.height.saturating_sub(3),
    );
    body
}
