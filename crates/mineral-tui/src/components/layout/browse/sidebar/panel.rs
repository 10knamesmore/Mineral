//! 左栏面板呈现：稳态绘制当前歌单或曲目列表，切换期间合成横向过渡帧。

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::render::theme::Theme;
use crate::runtime::state::AppState;

use super::{library, playlists, sweep};

/// 渲染左栏。过渡位置 `state.browse.view` 在端点时直接画对应单视图(零开销),中途则
/// 走 [`sweep`] 离屏合成出 Playlists ↔ Library 的横向过渡帧。
pub fn draw(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    let vp = &state.browse.view;
    if vp.at_min() {
        playlists::render_to(frame.buffer_mut(), area, state, theme);
    } else if vp.at_max() {
        library::render_to(frame.buffer_mut(), area, state, theme);
    } else {
        sweep::draw(
            frame.buffer_mut(),
            area,
            state,
            theme,
            vp.eased_in_out(),
            *state.cfg.tui().animation().view_sweep(),
        );
    }
}
