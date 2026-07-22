//! 右栏 Now Playing detail:Playlists 视图显示歌单 meta,Library 视图显示
//! 当前选中曲目 meta;一律包含程序化封面 + KV 区。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};
use ratatui_image::picker::Picker;

use crate::render::theme::Theme;
use crate::runtime::state::{AppState, View};

pub(crate) mod main_cover;
pub mod playlist;
pub mod track;

/// 渲染右栏。根据 [`AppState::view`] 选 playlist / track 详情。
///
/// # Params:
///   - `cover_in_flight`: page morph 封面飞行层已接管主封面时置真——面板跳过自画主图防双画
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    picker: &Picker,
    theme: &Theme,
    cover_in_flight: bool,
) {
    match state.browse.view.current() {
        View::Playlists => match state.selected_playlist() {
            Some(p) => playlist::draw(frame, area, p, state, picker, theme, cover_in_flight),
            None => paint_empty(frame, area, theme),
        },
        View::Library => {
            let tracks = state.filtered_tracks();
            match tracks.get(state.browse.nav.track.sel()) {
                Some(sv) => {
                    let current_id = state.playback.track.as_ref().map(|t| &t.id);
                    track::draw(
                        frame,
                        area,
                        sv,
                        current_id,
                        state,
                        picker,
                        theme,
                        cover_in_flight,
                    );
                }
                None => paint_empty(frame, area, theme),
            }
        }
    }
}

/// 没选中歌单 / 无 now-playing 时,渲染一个带标题的空 block 占位。
fn paint_empty(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(" selected ").style(Style::new().fg(theme.subtext)));
    frame.render_widget(block, area);
}
