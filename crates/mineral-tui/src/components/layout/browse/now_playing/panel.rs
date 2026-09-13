//! 右栏选中项详情：随左栏视图进度淡出淡入，排版固定；无选中项时绘制空面板。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};

use crate::components::layout::transition;
use crate::render::theme::Theme;
use crate::runtime::state::{AppState, View};

use super::{cover_transition, playlist, track};

/// 渲染右栏，端点直接画单态，途中按同一视图进度合成详情与封面。
///
/// # Params:
///   - `cover_in_flight`: 页面封面飞行层已接管时置真，只画详情文本
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    cover_in_flight: bool,
) {
    let view = &state.browse.view;
    if view.at_min() {
        draw_view(frame, area, state, theme, View::Playlists, cover_in_flight);
    } else if view.at_max() {
        draw_view(frame, area, state, theme, View::Library, cover_in_flight);
    } else {
        // 两端只画文字，封面在合成后统一绘制，避免离屏帧持有终端图片。
        let from = transition::capture(frame, area, |frame| {
            draw_view(frame, area, state, theme, View::Playlists, true);
        });
        let to = transition::capture(frame, area, |frame| {
            draw_view(frame, area, state, theme, View::Library, true);
        });
        transition::panel(frame, Some(&from), Some(&to), area, view.raw(), theme);
        if !cover_in_flight {
            cover_transition::draw(frame, area, state, theme, view.eased_in_out());
        }
    }
}

/// 按显式视图绘制一端，切换目标不会把两端都变成同一种详情。
fn draw_view(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    view: View,
    cover_in_flight: bool,
) {
    match view {
        View::Playlists => match state.selected_playlist() {
            Some(p) => playlist::draw(frame, area, p, state, theme, cover_in_flight),
            None => paint_empty(frame, area, theme),
        },
        View::Library => {
            let tracks = state.filtered_tracks();
            match tracks.get(state.browse.nav.track.sel()) {
                Some(sv) => {
                    let current_id = state.playback.track.as_ref().map(|t| &t.id);
                    track::draw(frame, area, sv, current_id, state, theme, cover_in_flight);
                }
                None => paint_empty(frame, area, theme),
            }
        }
    }
}

/// 没有选中歌单或曲目时，保留 selected 标题和边框。
fn paint_empty(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(" selected ").style(Style::new().fg(theme.subtext)));
    frame.render_widget(block, area);
}
