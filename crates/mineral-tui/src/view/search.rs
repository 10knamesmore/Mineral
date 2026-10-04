//! 搜索组件与共享模型、资源管线的接线。

use crate::components::frame::{FrameEnv, PrepareCx};
use crate::components::layout::search::detail::{DetailPaint, DetailView};
use crate::components::layout::search::{SearchView, detail, panel};
use crate::components::layout::shared::spinner;
use crate::image::ImageNeeds;
use crate::render::theme::Theme;
use crate::runtime::scroll::list::ScrollMotion;
use crate::runtime::state::{AppState, SearchFocus};
use ratatui::layout::Rect;

/// 结果和提示行只接收搜索组件本身及来源能力。
pub(crate) fn view<'a>(state: &'a AppState, theme: &'a Theme) -> SearchView<'a> {
    SearchView {
        page: &state.channel_search,
        caps: &state.caps,
        frame: FrameEnv {
            config: &state.cfg,
            theme,
            now: state.frame_now,
        },
        dock_right: state.browse.fullscreen.on(),
    }
}

/// 详情栈和正文输入，隔离搜索页其他会话及任务执行能力。
pub(crate) fn detail_view<'a>(state: &'a AppState, theme: &'a Theme) -> DetailView<'a> {
    DetailView {
        stack: state
            .channel_search
            .active_results()
            .map(|results| &results.detail),
        paint: DetailPaint {
            liked: &state.library.liked_ids,
            focus: state.channel_search.focus_permille(
                *state.cfg.tui().animation().search_focus_transition(),
                SearchFocus::Detail,
            ),
            loading: spinner::glyph(
                state.cfg.tui().animation().spinner_frames(),
                state.channel_search.spinner_counter(),
            ),
            frame: FrameEnv {
                config: &state.cfg,
                theme,
                now: state.frame_now,
            },
            images: state.images.ready(),
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
    let motion = if state.channel_search.active.at_max() {
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
            now: state.frame_now,
        },
        images: ImageNeeds::new(state.images.ready()),
        motion,
        image_phase: state.image_render_phase(),
        advance,
    };
    panel::prepare_results(left, &mut state.channel_search, &cx);
    if let Some(right) = right {
        detail::prepare(right, &mut state.channel_search, &mut cx, cover_in_flight);
    }
    state.images.reconcile(cx.images.finish());
}
