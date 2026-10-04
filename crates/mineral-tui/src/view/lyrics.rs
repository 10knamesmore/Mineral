//! 当前播放数据与歌词组件的接线。

use crate::components::frame::{FrameEnv, PrepareCx};
use crate::components::layout::browse::lyrics::{
    LyricMode, LyricsInput, LyricsPreparation, LyricsView,
};
use crate::components::lifecycle::ComponentView;
use crate::render::theme::Theme;
use crate::runtime::state::AppState;
use ratatui::{layout::Rect, style::Color};

/// 只借出当前歌曲的歌词与播放事实。
pub(crate) fn input<'a>(
    playback: &'a crate::runtime::playback::Playback,
    library: &'a crate::runtime::state::LibraryData,
) -> LyricsInput<'a> {
    let song = playback.track.as_ref().map(|song| &song.id);
    LyricsInput {
        song,
        lyrics: song.and_then(|song| library.lyrics.get(song)),
        position_ms: playback.position_ms,
        trust: playback.sync_trust(),
    }
}

/// 为绘制组合一份歌词视图。
pub(super) fn view<'a>(state: &'a AppState, theme: &'a Theme) -> ComponentView<'a, LyricsView<'a>> {
    let env = FrameEnv {
        config: &state.cfg,
        theme,
        now: state.ui.frame_now,
    };
    state.ui.browse.lyrics.bind_view(env, |panel| {
        panel.view(input(&state.models.playback, &state.models.library), env)
    })
}

/// 准备组件，更新范围仅为歌词面板自身。
pub(super) fn prepare(
    area: Rect,
    state: &mut AppState,
    theme: &Theme,
    mode: LyricMode,
    background: impl Fn(Rect) -> Color,
    advance: bool,
) {
    let input = input(&state.models.playback, &state.models.library);
    let frame = FrameEnv {
        config: &state.cfg,
        theme,
        now: state.ui.frame_now,
    };
    let mut cx = PrepareCx {
        frame,
        images: crate::image::ImageNeeds::new(state.resources.images.ready()),
        motion: crate::runtime::scroll::list::ScrollMotion::Frozen,
        image_phase: state.image_render_phase(),
        advance,
    };
    state.ui.browse.lyrics.prepare(
        area,
        LyricsPreparation {
            playback: input,
            mode,
            background: &background,
        },
        &mut cx,
    );
    state.resources.images.reconcile(cx.images.finish());
}
