//! 将浏览页选择连接到封面面板；资源需求交回图片管线。

use crate::components::frame::FrameEnv;
use crate::components::layout::browse::now_playing::{NowPlayingInput, NowPlayingView};
use crate::image::{ImageNeeds, ReadyImages};
use crate::render::theme::Theme;
use crate::runtime::playback::Playback;
use crate::runtime::state::BrowseModel;
use crate::runtime::state::{AppState, BrowsePage, LibraryData, View};
use mineral_model::MediaUrl;
#[cfg(test)]
use ratatui::Frame;
use ratatui::layout::Rect;

/// 选择只决定模型引用，不将整个浏览页借给面板。
fn input<'a>(
    browse: &BrowsePage,
    library: &'a LibraryData,
    playback: &'a Playback,
    cfg: &'a mineral_config::Config,
    images: ReadyImages<'_>,
) -> NowPlayingInput<'a> {
    let model = BrowseModel { library, cfg };
    let playlist = browse.selected_playlist_in_list(model);
    let track = browse
        .filtered_tracks(model)
        .get(browse.tracks.scroll().sel());
    let tracks = playlist.and_then(|p| library.tracks.get(&p.data.id));
    NowPlayingInput {
        playlist,
        track,
        playing: playback.track.as_ref().map(|song| &song.id),
        playlist_cover: playlist
            .and_then(|p| crate::image::collage::effective_cover_url(library, images, &p.data)),
        upcoming_cover: tracks
            .and_then(|tracks| tracks.first())
            .and_then(|entry| entry.data.song.cover_url.clone()),
        duration_ms: tracks.and_then(|tracks| {
            tracks
                .iter()
                .filter_map(|entry| entry.data.song.duration_ms)
                .reduce(|total, duration| total + duration)
        }),
        switch: browse.view,
    }
}

/// 构造只读面板。
pub(crate) fn view<'a>(state: &'a AppState, theme: &'a Theme) -> NowPlayingView<'a> {
    NowPlayingView {
        panel: &state.ui.browse.now_playing,
        input: input(
            &state.ui.browse,
            &state.models.library,
            &state.models.playback,
            &state.cfg,
            state.resources.images.ready(),
        ),
        frame: FrameEnv {
            config: &state.cfg,
            theme,
            now: state.ui.frame_now,
        },
        images: state.resources.images.ready(),
        phase: state.image_render_phase(),
    }
}

/// 准备局部状态与图片需求。
pub(crate) fn prepare(area: Rect, state: &mut AppState, cover_in_flight: bool) {
    let input = input(
        &state.ui.browse,
        &state.models.library,
        &state.models.playback,
        &state.cfg,
        state.resources.images.ready(),
    );
    let mut images = ImageNeeds::new(state.resources.images.ready());
    let phase = state.image_render_phase();
    state.ui.browse.now_playing.prepare(
        area,
        &input,
        state.ui.frame_now,
        &mut images,
        phase,
        cover_in_flight,
    );
    state.resources.images.reconcile(images.finish());
}

/// 绘制当前选择的面板。
#[cfg(test)]
pub(crate) fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    cover_in_flight: bool,
) {
    crate::components::layout::browse::now_playing::draw(
        frame,
        area,
        &view(state, theme),
        theme,
        cover_in_flight,
    );
}

/// 返回面板当前主封面的 URL。
pub(crate) fn url(state: &AppState) -> Option<MediaUrl> {
    url_for_view(state, state.ui.browse.view.current())
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
            crate::image::collage::effective_cover_url(
                &state.models.library,
                state.resources.images.ready(),
                &playlist.data,
            )
        }
        View::Library => {
            let tracks = state.filtered_tracks();
            let song = &tracks.get(state.ui.browse.tracks.scroll().sel())?.data.song;
            song.cover_url.clone()
        }
    }
}
