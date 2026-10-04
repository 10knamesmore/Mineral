//! 浏览列表的布局准备与共用几何。

use ratatui::layout::Rect;

use crate::render::theme::Theme;
use crate::runtime::scroll::list::ScrollMotion;
use crate::runtime::state::AppState;

use super::{library, playlists};

/// 准备页面实际参与合成的列表，离屏端点只声明需求，不推进滚动。
pub(crate) fn prepare(area: Rect, state: &mut AppState, theme: &Theme, advance: bool) {
    if !state.browse.view.at_max() {
        playlists::prepare(area, state, theme, advance);
    }
    if !state.browse.view.at_min() {
        library::prepare(area, state, theme, advance);
    }
}

/// 页面形变中的列表保留其稳定视口目标。
pub(super) fn motion(state: &AppState) -> ScrollMotion {
    if state.browse.fullscreen.at_min()
        && state.channel_search.active.at_min()
        && (state.browse.view.at_min() || state.browse.view.at_max())
    {
        ScrollMotion::Advancing {
            scrolloff: state.scrolloff(),
            glide_ticks: state.list_glide_ticks(),
        }
    } else {
        ScrollMotion::Frozen
    }
}

/// 列表右边框的 minimap 轨道，包含表头与底部计数行。
pub(super) fn minimap_track(area: Rect) -> Rect {
    Rect::new(
        area.right().saturating_sub(1),
        area.y.saturating_add(1),
        area.width.min(1),
        area.height.saturating_sub(2),
    )
}
