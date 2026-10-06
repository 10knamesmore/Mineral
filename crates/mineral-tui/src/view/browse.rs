//! 浏览页面与共享模型的接线；子组件只接收自己的数据与准备能力。

use crate::components::frame::{FrameEnv, PrepareCx};
use crate::components::layout::browse::sidebar::{
    library::{TrackInput, TrackView},
    playlists::{PlaylistInput, PlaylistView},
    sweep,
};
use crate::components::layout::shared::thumbnails::thumbnail_phase;
use crate::components::lifecycle::ComponentView;
use crate::image::ImageNeeds;
use crate::render::theme::Theme;
use crate::runtime::state::AppState;
use ratatui::{Frame, layout::Rect};

/// 借出当前曲目列表所需的数据，不把应用状态传入组件。
pub(crate) fn tracks<'a>(
    state: &'a AppState,
    theme: &'a Theme,
) -> ComponentView<'a, TrackView<'a>> {
    let playlist = state.opened_playlist();
    let input = TrackInput {
        playlist,
        tracks: playlist.and_then(|playlist| state.models.library.tracks.get(&playlist.data.id)),
        generation: state.models.library.tracks_generation,
        playing: state.models.playback.track.as_ref().map(|song| &song.id),
    };
    let env = FrameEnv {
        config: &state.cfg,
        theme,
        now: state.ui.frame_now,
    };
    state.ui.browse.tracks.bind_view(env, |list| {
        list.view(input, env, state.resources.images.ready())
    })
}

/// 借出歌单模型与加载状态。
pub(crate) fn playlists<'a>(
    state: &'a AppState,
    theme: &'a Theme,
) -> ComponentView<'a, PlaylistView<'a>> {
    let env = FrameEnv {
        config: &state.cfg,
        theme,
        now: state.ui.frame_now,
    };
    state.ui.browse.playlists.bind_view(env, |list| {
        list.view(
            PlaylistInput {
                library: &state.models.library,
                loading: state.models.tasks_snapshot.running > 0,
            },
            env,
            state.resources.images.ready(),
        )
    })
}

/// 页面形变中的列表保留稳定视口目标。
fn motion(state: &AppState) -> crate::runtime::scroll::list::ScrollMotion {
    use crate::runtime::scroll::list::ScrollMotion;
    if state.ui.browse.fullscreen.at_min()
        && state.ui.channel_search.active.at_min()
        && (state.ui.browse.view.at_min() || state.ui.browse.view.at_max())
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
        state.ui.frame_now,
        state.ui.browse.nav.last_sel_change,
        std::time::Duration::from_millis(*state.cfg.cover().debounce_ms()),
    );
    let mut cx = PrepareCx {
        frame: FrameEnv {
            config: &state.cfg,
            theme,
            now: state.ui.frame_now,
        },
        images: ImageNeeds::new(state.resources.images.ready()),
        motion,
        image_phase,
        advance,
    };
    if !state.ui.browse.view.at_max() {
        state.ui.browse.playlists.prepare(
            area,
            PlaylistInput {
                library: &state.models.library,
                loading: state.models.tasks_snapshot.running > 0,
            },
            &mut cx,
        );
    }
    if !state.ui.browse.view.at_min() {
        let playlist = state.ui.browse.nav.opened_playlist.as_ref().and_then(|id| {
            state
                .models
                .library
                .playlists
                .iter()
                .find(|playlist| &playlist.data.id == id)
        });
        let input = TrackInput {
            playlist,
            tracks: playlist
                .and_then(|playlist| state.models.library.tracks.get(&playlist.data.id)),
            generation: state.models.library.tracks_generation,
            playing: state.models.playback.track.as_ref().map(|song| &song.id),
        };
        state.ui.browse.tracks.prepare(area, input, &mut cx);
    }
    state.resources.images.reconcile(cx.images.finish());
}

/// 浏览页组合：分别借入两个子列表视图。
pub(crate) struct BrowseView<'a> {
    /// 歌单列表。
    playlists: ComponentView<'a, PlaylistView<'a>>,

    /// 曲目列表。
    tracks: ComponentView<'a, TrackView<'a>>,

    /// 两端共用的过渡位置。
    switch: crate::runtime::state::ViewSwitch,

    /// 当前配置的横向过渡风格。
    sweep: crate::config::SweepStyle,
}

/// 根视图借出浏览页的组件输入。
pub(crate) fn view<'a>(state: &'a AppState, theme: &'a Theme) -> BrowseView<'a> {
    BrowseView {
        playlists: playlists(state, theme),
        tracks: tracks(state, theme),
        switch: state.ui.browse.view,
        sweep: *state.cfg.animation().view_sweep(),
    }
}

impl BrowseView<'_> {
    /// 稳态直接使用已有列表实例；切换期间由页面合成两端内容。
    pub(crate) fn plan<'a>(&'a self, area: Rect, plan: &mut crate::render::memo::FramePlan<'a>) {
        if self.switch.at_min() {
            plan.add(area, self.playlists.as_ref());
        } else if self.switch.at_max() {
            plan.add(area, self.tracks.as_ref());
        } else {
            plan.moving(move |frame| self.paint(frame, area));
        }
    }

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
