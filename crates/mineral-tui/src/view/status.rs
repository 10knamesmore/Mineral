//! 顶栏只读输入的组合。

use crate::components::layout::shared::top_status::StatusInput;
use crate::runtime::state::AppState;

/// 读取当前任务计数和播放状态。
pub(super) fn input(state: &AppState) -> StatusInput<'_> {
    StatusInput {
        view: state.browse.view.current(),
        dim: state.dim.eased_in_out(),
        playback: &state.playback,
        tasks: &state.tasks_snapshot,
        loading_images: state.images.loading_count(),
    }
}
