//! 当前播放数据与歌词组件的接线。

use crate::components::frame::FrameEnv;
use crate::components::layout::browse::lyrics::{LyricMode, LyricsInput, LyricsView};
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
pub(super) fn view<'a>(state: &'a AppState, theme: &'a Theme) -> LyricsView<'a> {
    state.browse.lyrics.view(
        input(&state.playback, &state.library),
        FrameEnv {
            config: &state.cfg,
            theme,
            now: state.frame_now,
        },
    )
}

/// 准备组件，更新范围仅为歌词面板自身。
pub(super) fn prepare(
    area: Rect,
    state: &mut AppState,
    theme: &Theme,
    mode: LyricMode,
    background: impl Fn(Rect) -> Color,
) {
    let input = input(&state.playback, &state.library);
    let frame = FrameEnv {
        config: &state.cfg,
        theme,
        now: state.frame_now,
    };
    state
        .browse
        .lyrics
        .prepare(area, input, frame, mode, background);
}
