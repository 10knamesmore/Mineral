//! 主视图的准备与只读借用：按页面组合调用组件准备，终端提交由主循环单独执行。

use crate::app::App;
use crate::components::layout::browse::{lyrics, now_playing, sidebar, spectrum};
use crate::components::layout::flight;
use crate::components::layout::search::{detail, panel};
use crate::components::layout::shared::compute::{compute, compute_fullscreen, compute_search};
use crate::components::layout::shared::{transform, transport};
use crate::image::{ImageContent, ImageRenderPhase};
use crate::render::ambient::{self, AmbientField};
use crate::runtime::state::AppState;
use ratatui::layout::{Position, Rect};
use std::time::Instant;

/// 绘制所需的只读状态；不包含后端连接、任务提交或终端输出能力。
pub(crate) struct FrameView<'a> {
    /// 已完成布局准备的界面与数据镜像。
    pub(super) state: &'a AppState,

    /// 本次有效主题。
    pub(super) theme: &'a crate::render::theme::Theme,

    /// 已采样的氛围动画。
    pub(super) ambient: &'a ambient::AmbientGradient,

    /// 本拍响度包络。
    pub(super) ambient_pulse: &'a ambient::LoudnessPulse,

    /// 浮层的只读显示状态。
    pub(super) overlays: &'a crate::components::popup::OverlayStack,

    /// 已更新生命周期的通知。
    pub(super) notifications: &'a crate::components::toast::notifications::Notifications,

    /// 通知关闭键提示。
    pub(super) notice_hint: &'a str,

    /// 启动或退出的显示进度。
    pub(super) transition: &'a Option<crate::render::anim::Transition>,

    /// 整屏动画锚点。
    pub(super) launch_anchor: Option<Position>,
}

impl App {
    /// 根据当前尺寸准备一份可独立重复或省略绘制的显示状态。
    /// `advance` 只在逻辑时钟走过一拍时为真；输入、resize 和资源回填不额外推进动画。
    pub(crate) fn prepare_view(&mut self, area: Rect, now: Instant, advance: bool) {
        self.state.frame_now = now;
        self.state.frame_area = area;
        self.state.images.begin_preparation();
        let layout = self.state.cfg.tui().layout();
        let normal = compute(area, layout);
        let full = compute_fullscreen(area, layout);
        let search = compute_search(area, layout);
        let ambient_cfg = self.state.cfg.tui().ambient();
        let field = (self.state.browse.ambient_reveal.active()
            && (*ambient_cfg.enabled() || !self.ambient.settled_at_base()))
        .then(|| ambient::rgb_of(self.theme.base))
        .flatten()
        .map(|base| {
            AmbientField::new(
                area,
                &self.ambient,
                base,
                ambient_cfg,
                self.state.browse.ambient_reveal.progress(),
                self.ambient_pulse.level_permille(ambient_cfg.pulse()),
            )
        });
        let background = self.theme.background;
        let sample = |rect: Rect| {
            field.as_ref().map_or(background, |field| {
                field.color_at(rect.x + rect.width / 2, rect.y + rect.height / 2)
            })
        };
        if !self.state.browse.fullscreen.at_min() {
            if self.state.browse.fullscreen.at_max() {
                if let Some(cover) = full.cover {
                    prepare_fullscreen_cover(cover, full.cover, &mut self.state);
                }
                if let Some(lyric) = full.lyrics {
                    lyrics::prepare(
                        lyric,
                        &mut self.state,
                        &self.theme,
                        lyrics::LyricMode::Immersive,
                        sample,
                    );
                }
                if let Some(spectrum) = full.spectrum {
                    spectrum::prepare(spectrum, &mut self.state.spectrum);
                }
                transport::prepare(full.transport, &mut self.state);
            } else {
                let current = transform::morph_areas(
                    &normal,
                    &full,
                    self.state.browse.fullscreen.eased_in_out(),
                );
                let flight = flight::plan_fullscreen(&normal, &full, &self.state);
                sidebar::prepare(normal.left, &mut self.state, &self.theme, advance);
                if let Some(right) = normal.right {
                    now_playing::prepare(right, &mut self.state, flight.is_some());
                }
                if let Some(plan) = flight {
                    flight::prepare(&plan, &mut self.state);
                } else if let Some(cover) = current.cover {
                    prepare_fullscreen_cover(cover, full.cover, &mut self.state);
                }
                if let Some(lyric) = normal.lyrics {
                    lyrics::prepare(
                        lyric,
                        &mut self.state,
                        &self.theme,
                        lyrics::LyricMode::Compact,
                        sample,
                    );
                }
                if let Some(lyric) = full.lyrics {
                    lyrics::prepare(
                        lyric,
                        &mut self.state,
                        &self.theme,
                        lyrics::LyricMode::Immersive,
                        sample,
                    );
                }
                if let Some(spectrum) = current.spectrum {
                    spectrum::prepare(spectrum, &mut self.state.spectrum);
                }
                transport::prepare(current.transport, &mut self.state);
            }
        } else if !self.state.channel_search.active.at_min() {
            let moving = !self.state.channel_search.active.at_max();
            let flight = moving
                .then(|| flight::plan(&normal, &search, &self.state))
                .flatten();
            if moving {
                sidebar::prepare(normal.left, &mut self.state, &self.theme, advance);
                if let Some(right) = normal.right {
                    now_playing::prepare(right, &mut self.state, flight.is_some());
                }
                if let Some(lyric) = normal.lyrics {
                    lyrics::prepare(
                        lyric,
                        &mut self.state,
                        &self.theme,
                        lyrics::LyricMode::Compact,
                        sample,
                    );
                }
                if let Some(spectrum) = normal.spectrum {
                    spectrum::prepare(spectrum, &mut self.state.spectrum);
                }
            }
            panel::prepare_results(search.left, &mut self.state, advance);
            if let Some(right) = search.right {
                detail::prepare(
                    right,
                    &mut self.state,
                    &self.theme,
                    flight.is_some(),
                    advance,
                );
            }
            if let Some(plan) = flight {
                flight::prepare(&plan, &mut self.state);
            }
            let current = transform::morph_search(
                &normal,
                &search,
                self.state.channel_search.active.eased_in_out(),
            );
            transport::prepare(current.transport, &mut self.state);
        } else {
            sidebar::prepare(normal.left, &mut self.state, &self.theme, advance);
            if let Some(right) = normal.right {
                now_playing::prepare(right, &mut self.state, false);
            }
            if let Some(lyric) = normal.lyrics {
                lyrics::prepare(
                    lyric,
                    &mut self.state,
                    &self.theme,
                    lyrics::LyricMode::Compact,
                    sample,
                );
            }
            if let Some(spectrum) = normal.spectrum {
                spectrum::prepare(spectrum, &mut self.state.spectrum);
            }
            transport::prepare(normal.transport, &mut self.state);
        }
        if self.transition.is_none() {
            self.overlays
                .prepare(area, &mut self.state, &self.theme, advance);
        }
        self.state.images.finish_preparation();
    }

    /// 只借出绘制所需的状态；该借用存续期间不能更新应用。
    pub(crate) fn frame_view(&self) -> FrameView<'_> {
        FrameView {
            state: &self.state,
            theme: &self.theme,
            ambient: &self.ambient,
            ambient_pulse: &self.ambient_pulse,
            overlays: &self.overlays,
            notifications: &self.notifications,
            notice_hint: &self.notice_hint,
            transition: &self.transition,
            launch_anchor: self.launch_anchor,
        }
    }
}

/// 全屏封面准备：实际显示保活与稳定尺寸预热分别声明。
fn prepare_fullscreen_cover(area: Rect, steady: Option<Rect>, state: &mut AppState) {
    let url = state
        .playback
        .track
        .as_ref()
        .and_then(|track| track.cover_url.clone());
    if state.browse.fullscreen.at_max() {
        if let Some(active) = &state.images.transition {
            let from = active.from_url.clone();
            let to = active.to_url.clone();
            let progress = active.anim.eased_in_out();
            let advance = active.advance;
            let style = crate::image::BlendStyle::from(*state.cfg.tui().cover_transition().style());
            state.images.prepare_display(
                ImageContent::Blend {
                    from: &from,
                    to: &to,
                    progress,
                    style,
                    advance,
                },
                area,
                ImageRenderPhase::Stable,
            );
            state.images.prepare(&to, area);
        } else {
            state.images.prepare_display(
                ImageContent::Display { url: url.as_ref() },
                area,
                ImageRenderPhase::Stable,
            );
        }
        let urls = state
            .queue_neighbor_indexes(*state.cfg.tui().prefetch().prewarm_ahead())
            .into_iter()
            .filter_map(|index| {
                state
                    .player
                    .queue
                    .get(index)
                    .and_then(|song| song.cover_url.clone())
            })
            .collect::<Vec<_>>();
        for url in urls {
            state.images.prepare(&url, area);
        }
    } else {
        state.images.prepare_display(
            ImageContent::Display { url: url.as_ref() },
            area,
            ImageRenderPhase::Resizing,
        );
        if state.browse.fullscreen.on()
            && let (Some(url), Some(steady)) = (&url, steady)
        {
            state.images.prepare(url, steady);
        }
    }
}
