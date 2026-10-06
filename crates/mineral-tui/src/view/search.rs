//! 搜索组件与共享模型、资源管线的接线。

use crate::components::frame::{FrameEnv, PrepareCx};
use crate::components::layout::search::detail::{DetailPaint, DetailView};
use crate::components::layout::search::{SearchPreparation, SearchView};
use crate::components::layout::shared::spinner;
use crate::components::lifecycle::ComponentView;
use crate::image::ImageNeeds;
use crate::render::theme::Theme;
use crate::runtime::scroll::list::ScrollMotion;
use crate::runtime::state::{AppState, SearchFocus};
use ratatui::layout::Rect;

/// 结果和提示行只接收搜索组件本身及来源能力。
pub(crate) fn view<'a>(state: &'a AppState, theme: &'a Theme) -> ComponentView<'a, SearchView<'a>> {
    let env = FrameEnv {
        config: &state.cfg,
        theme,
        now: state.ui.frame_now,
    };
    state.ui.channel_search.bind_view(env, |page| SearchView {
        page,
        caps: &state.models.caps,
        frame: env,
        dock_right: state.ui.browse.fullscreen.on(),
    })
}

/// 详情栈和正文输入，隔离搜索页其他会话及任务执行能力。
pub(crate) fn detail_view<'a>(state: &'a AppState, theme: &'a Theme) -> DetailView<'a> {
    DetailView {
        stack: state
            .ui
            .channel_search
            .active_results()
            .map(|results| &results.detail),
        paint: DetailPaint {
            liked: &state.models.library.liked_ids,
            focus: state.ui.channel_search.focus_permille(
                *state.cfg.animation().search_focus_transition(),
                SearchFocus::Detail,
            ),
            loading: spinner::glyph(
                state.cfg.animation().spinner_frames(),
                state.ui.channel_search.spinner_counter(),
            ),
            frame: FrameEnv {
                config: &state.cfg,
                theme,
                now: state.ui.frame_now,
            },
            images: state.resources.images.ready(),
            phase: state.image_render_phase(),
        },
    }
}

/// 准备当前结果及可见详情，不让绘制负责滚动或资源调度。
pub(super) fn prepare(
    left: Rect,
    right: Option<Rect>,
    state: &mut AppState,
    theme: &Theme,
    cover_in_flight: bool,
    advance: bool,
) {
    let motion = if state.ui.channel_search.active.at_max() {
        ScrollMotion::Advancing {
            scrolloff: state.scrolloff(),
            glide_ticks: state.list_glide_ticks(),
        }
    } else {
        ScrollMotion::Frozen
    };
    let mut cx = PrepareCx {
        frame: FrameEnv {
            config: &state.cfg,
            theme,
            now: state.ui.frame_now,
        },
        images: ImageNeeds::new(state.resources.images.ready()),
        motion,
        image_phase: state.image_render_phase(),
        advance,
    };
    state.ui.channel_search.prepare(
        left,
        SearchPreparation {
            detail_area: right,
            cover_in_flight,
        },
        &mut cx,
    );
    state.resources.images.reconcile(cx.images.finish());
}
