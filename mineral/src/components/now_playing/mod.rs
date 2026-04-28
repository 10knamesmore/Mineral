//! 右栏 Now Playing detail:Playlists 视图显示歌单 meta,Library 视图显示
//! 当前选中曲目 meta;一律包含程序化封面 + KV 区。

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};
use ratatui::Frame;

use crate::state::{AppState, View};
use crate::theme::Theme;

pub mod playlist_detail;
pub mod track_detail;

/// 渲染右栏。根据 [`AppState::view`] 选 playlist_detail / track_detail。
pub fn draw(frame: &mut Frame<'_>, area: Rect, state: &AppState, theme: &Theme) {
    match state.view {
        View::Playlists => match state.selected_playlist() {
            Some(p) => playlist_detail::draw(frame, area, p, theme),
            None => paint_empty(frame, area, theme),
        },
        View::Library => {
            let tracks = state.filtered_tracks();
            match tracks.get(state.sel_track) {
                Some(sv) => {
                    let current_id = state.playback.track.as_ref().map(|t| &t.id);
                    track_detail::draw(frame, area, sv, current_id, theme);
                }
                None => paint_empty(frame, area, theme),
            }
        }
    }
}

fn paint_empty(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(" now playing ").style(Style::new().fg(theme.subtext)));
    frame.render_widget(block, area);
}
