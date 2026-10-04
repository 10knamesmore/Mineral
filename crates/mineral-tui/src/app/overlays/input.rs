//! 把应用模型投影为各浮层需要的数据。

use crate::components::frame::FrameEnv;
use crate::components::popup::{DownloadInput, OverlayEnv, QueueInput, dock_full_rect};
use crate::render::theme::Theme;
use crate::runtime::state::AppState;

/// 为通用浮层容器借出布局配置与显示环境。
pub(crate) fn environment<'a>(state: &'a AppState, theme: &'a Theme) -> OverlayEnv<'a> {
    OverlayEnv {
        frame: FrameEnv {
            config: &state.cfg,
            theme,
            now: state.ui.frame_now,
        },
        dock_right: state.ui.browse.fullscreen.on(),
    }
}

/// 队列不借用整个播放器或歌曲库，只接收本面板实际使用的事实。
pub(crate) fn queue(state: &AppState) -> QueueInput<'_> {
    QueueInput {
        queue: &state.models.player.queue,
        current: state.queue_current_index(),
        attached: state.models.player.cursor.is_attached(),
        position_ms: state.models.playback.position_ms,
        playing: state.models.playback.playing,
        liked: &state.models.library.liked_ids,
        cfg: &state.cfg,
        now: state.ui.now,
        frame_now: state.ui.frame_now,
        panel_area: dock_full_rect(
            state.ui.frame_area,
            state.cfg.tui().layout(),
            state.ui.browse.fullscreen.on(),
        ),
        images: state.resources.images.ready(),
    }
}

/// 下载面板只借用当前明细和汇总。
pub(crate) fn downloads(state: &AppState) -> DownloadInput<'_> {
    DownloadInput {
        downloads: &state.models.downloads,
        downloads_summary: &state.models.downloads_summary,
        cfg: &state.cfg,
        frame_now: state.ui.frame_now,
    }
}

/// 应用挂载的浮层输入；容器仅将其转发，叶子只取得自己的投影。
pub(crate) struct OverlayInputs<'a> {
    /// 队列面板输入。
    pub(crate) queue: QueueInput<'a>,

    /// 下载面板输入。
    pub(crate) downloads: DownloadInput<'a>,

    /// 当前音频输出。
    pub(crate) output: Option<&'a mineral_audio::AudioOutput>,

    /// 当前行为配置。
    pub(crate) behavior: &'a mineral_config::BehaviorConfig,
}

/// 为浮层事件或绘制建立只读输入集合。
pub(crate) fn all(state: &AppState) -> OverlayInputs<'_> {
    OverlayInputs {
        queue: queue(state),
        downloads: downloads(state),
        output: state.models.playback.output.as_deref(),
        behavior: state.cfg.tui().behavior(),
    }
}
