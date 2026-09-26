//! now_playing 面板主封面槽的单一源:内区纵切几何 + 当前主封面身份。
//! track / playlist 绘制与 page morph 封面飞行层共用,保证飞行端点与面板实画零漂移。

use mineral_model::MediaUrl;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Block, Borders};

use crate::runtime::state::{AppState, View};

/// now_playing 内区(去边框)纵切三段:上 cover / 中 2 行 KV / 底 1 行。内区画不下
/// (过窄 / 过矮)为 `None`,与面板绘制的早退同一阈值。
///
/// # Params:
///   - `area`: 面板整区(含边框)
///
/// # Return:
///   `[cover, kv, 底行]`;画不下为 `None`。
pub(crate) fn sections(area: Rect) -> Option<[Rect; 3]> {
    let inner = Block::new().borders(Borders::ALL).inner(area);
    if inner.height < 4 || inner.width < 8 {
        return None;
    }
    Some(
        Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .areas(inner),
    )
}

/// 返回面板当前主封面的 URL。
pub(crate) fn url(state: &AppState) -> Option<MediaUrl> {
    url_for_view(state, state.browse.view.current())
}

/// 返回指定视图的选中项封面，供过渡两端独立取图。
///
/// # Params:
///   - `state`: 当前歌单和曲目选择
///   - `view`: Playlists 取歌单封面，Library 取选中曲目封面
pub(crate) fn url_for_view(state: &AppState, view: View) -> Option<MediaUrl> {
    match view {
        View::Playlists => {
            let playlist = state.selected_playlist_in_list()?;
            crate::image::collage::effective_cover_url(state, &playlist.data)
        }
        View::Library => {
            let tracks = state.filtered_tracks();
            let song = &tracks.get(state.browse.nav.track.sel())?.data.song;
            song.cover_url.clone()
        }
    }
}
