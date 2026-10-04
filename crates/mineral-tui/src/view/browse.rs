//! 浏览页面与共享模型的接线；子组件只接收自己的数据与准备能力。

use crate::components::frame::{FrameEnv, PrepareCx};
use crate::components::layout::browse::sidebar::{
    library::{TrackInput, TrackView},
    playlists::{PlaylistInput, PlaylistView},
    sweep,
};
use crate::components::layout::shared::thumbnails::thumbnail_phase;
use crate::image::ImageNeeds;
use crate::render::theme::Theme;
use crate::runtime::state::AppState;
use ratatui::{Frame, layout::Rect};

/// 借出当前曲目列表所需的数据，不把应用状态传入组件。
pub(crate) fn tracks<'a>(state: &'a AppState, theme: &'a Theme) -> TrackView<'a> {
    let playlist = state.opened_playlist();
    let input = TrackInput {
        playlist,
        tracks: playlist.and_then(|playlist| state.library.tracks.get(&playlist.data.id)),
        generation: state.library.tracks_generation,
        playing: state.playback.track.as_ref().map(|song| &song.id),
    };
    state.browse.tracks.view(
        input,
        FrameEnv {
            config: &state.cfg,
            theme,
            now: state.frame_now,
        },
        state.images.ready(),
    )
}

/// 借出歌单模型与加载状态。
pub(crate) fn playlists<'a>(state: &'a AppState, theme: &'a Theme) -> PlaylistView<'a> {
    state.browse.playlists.view(
        PlaylistInput {
            library: &state.library,
            loading: state.tasks_snapshot.running > 0,
        },
        FrameEnv {
            config: &state.cfg,
            theme,
            now: state.frame_now,
        },
        state.images.ready(),
    )
}

/// 页面形变中的列表保留稳定视口目标。
fn motion(state: &AppState) -> crate::runtime::scroll::list::ScrollMotion {
    use crate::runtime::scroll::list::ScrollMotion;
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

/// 组合可见列表的准备，再把资源需求交回图片管线。
pub(crate) fn prepare(area: Rect, state: &mut AppState, theme: &Theme, advance: bool) {
    let motion = motion(state);
    let image_phase = thumbnail_phase(
        state.image_render_phase(),
        motion,
        state.frame_now,
        state.browse.nav.last_sel_change,
        std::time::Duration::from_millis(*state.cfg.tui().cover().debounce_ms()),
    );
    let mut cx = PrepareCx {
        frame: FrameEnv {
            config: &state.cfg,
            theme,
            now: state.frame_now,
        },
        images: ImageNeeds::new(state.images.ready()),
        motion,
        image_phase,
        advance,
    };
    if !state.browse.view.at_max() {
        state.browse.playlists.prepare(
            area,
            PlaylistInput {
                library: &state.library,
                loading: state.tasks_snapshot.running > 0,
            },
            &mut cx,
        );
    }
    if !state.browse.view.at_min() {
        let playlist = state.browse.nav.opened_playlist.as_ref().and_then(|id| {
            state
                .library
                .playlists
                .iter()
                .find(|playlist| &playlist.data.id == id)
        });
        let input = TrackInput {
            playlist,
            tracks: playlist.and_then(|playlist| state.library.tracks.get(&playlist.data.id)),
            generation: state.library.tracks_generation,
            playing: state.playback.track.as_ref().map(|song| &song.id),
        };
        state.browse.tracks.prepare(area, input, &mut cx);
    }
    state.images.reconcile(cx.images.finish());
}

/// 浏览页组合：分别借入两个子列表视图。
pub(crate) struct BrowseView<'a> {
    /// 歌单列表。
    playlists: PlaylistView<'a>,

    /// 曲目列表。
    tracks: TrackView<'a>,

    /// 两端共用的过渡位置。
    switch: crate::runtime::state::ViewSwitch,

    /// 当前配置的横向过渡风格。
    sweep: mineral_config::SweepStyle,
}

/// 根视图借出浏览页的组件输入。
pub(crate) fn view<'a>(state: &'a AppState, theme: &'a Theme) -> BrowseView<'a> {
    BrowseView {
        playlists: playlists(state, theme),
        tracks: tracks(state, theme),
        switch: state.browse.view,
        sweep: *state.cfg.tui().animation().view_sweep(),
    }
}

impl BrowseView<'_> {
    /// 两个列表只画自己的内容，页面负责组合过渡。
    pub(crate) fn paint(&self, frame: &mut Frame<'_>, area: Rect) {
        if self.switch.at_min() {
            self.playlists.paint(area, frame.buffer_mut());
        } else if self.switch.at_max() {
            self.tracks.paint(area, frame.buffer_mut());
        } else {
            sweep::draw(
                frame.buffer_mut(),
                area,
                |buf| self.playlists.paint(area, buf),
                |buf| self.tracks.paint(area, buf),
                self.switch.eased_in_out(),
                self.sweep,
            );
        }
    }
}
