//! 组合根为播放栏借出播放模型与本帧环境。

use crate::components::frame::FrameEnv;
use crate::components::layout::shared::transport::{TransportInput, TransportView};
use crate::render::theme::Theme;
use crate::runtime::state::AppState;

/// 播放栏不接收应用状态；这里完成共享模型的投影。
pub(super) fn view<'a>(state: &'a AppState, theme: &'a Theme) -> TransportView<'a> {
    state.ui.transport.view(
        TransportInput {
            playback: &state.models.playback,
            palette: state.resources.images.current_palette.as_ref(),
        },
        FrameEnv {
            config: &state.cfg,
            theme,
            now: state.ui.frame_now,
        },
    )
}
