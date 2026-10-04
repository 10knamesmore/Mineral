//! 组合根为播放栏借出播放模型与本帧环境。

use crate::components::frame::FrameEnv;
use crate::components::layout::shared::transport::{TransportInput, TransportView};
use crate::components::lifecycle::ComponentView;
use crate::render::theme::Theme;
use crate::runtime::state::AppState;

/// 播放栏不接收应用状态；这里完成共享模型的投影。
pub(super) fn view<'a>(
    state: &'a AppState,
    theme: &'a Theme,
) -> ComponentView<'a, TransportView<'a>> {
    let env = FrameEnv {
        config: &state.cfg,
        theme,
        now: state.ui.frame_now,
    };
    state.ui.transport.bind_view(env, |bar| {
        bar.view(
            TransportInput {
                playback: &state.models.playback,
            },
            env,
        )
    })
}
