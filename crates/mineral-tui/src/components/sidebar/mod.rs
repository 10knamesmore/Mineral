//! 左栏:Playlists / Library 双视图渲染入口。

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::state::{AppState, View};
use crate::theme::Theme;

mod highlight;
pub mod library;
pub mod playlists;

/// 根据 [`AppState::view`] 选择对应渲染器。
pub fn draw(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    match state.view {
        View::Playlists => playlists::draw(frame, area, state, theme),
        View::Library => library::draw(frame, area, state, theme),
    }
}
