//! 主视图的准备与只读借用：按页面组合调用组件准备，终端提交由主循环单独执行。

use crate::app::App;
use crate::components::layout::browse::{lyrics, spectrum};
use crate::components::layout::flight;
use crate::components::layout::shared::compute::{compute, compute_fullscreen, compute_search};
use crate::components::layout::shared::transform;
use crate::image::{ImageContent, ImageRenderPhase};
use crate::render::ambient::{self, AmbientField};
use crate::runtime::state::AppState;
use ratatui::layout::{Position, Rect};
use std::time::Instant;

/// 绘制所需的只读状态；不包含后端连接、任务提交或终端输出能力。
pub(crate) struct FrameView<'a> {
    /// 本帧环境。
    pub(super) env: crate::components::frame::FrameEnv<'a>,

    /// 浏览页的两个列表视图。
    pub(super) browse: super::browse::BrowseView<'a>,

    /// 选中项详情视图。
    pub(super) selected: crate::components::layout::browse::now_playing::NowPlayingView<'a>,

    /// 歌词组件视图。
    pub(super) lyrics: crate::components::layout::browse::lyrics::LyricsView<'a>,

    /// 搜索结果与输入视图。
    pub(super) search: crate::components::layout::search::SearchView<'a>,

    /// 当前详情栈。
    pub(super) detail: crate::components::layout::search::detail::DetailView<'a>,

    /// 播放栏视图。
    pub(super) transport: crate::components::layout::shared::transport::TransportView<'a>,

    /// 顶栏事实。
    pub(super) status: crate::components::layout::shared::top_status::StatusInput<'a>,

    /// 频谱的本帧显示状态。
    pub(super) spectrum: &'a crate::components::layout::browse::spectrum::SpectrumState,

    /// 全屏页面过渡。
    pub(super) fullscreen: &'a crate::render::anim::Toggle,

    /// 氛围背景的揭示进度。
    pub(super) ambient_reveal: &'a crate::render::anim::TrailingToggle,

    /// 已就绪图片资源。
    pub(super) images: crate::image::ReadyImages<'a>,

    /// 在播歌曲，供全屏主封面使用。
    pub(super) playing: Option<&'a mineral_model::Song>,

    /// 全屏切歌封面的既定过渡。
    pub(super) cover_transition: Option<&'a crate::image::CoverTransition>,

    /// 待机唱片的既定旋转相位。
    pub(super) vinyl: &'a crate::components::layout::shared::vinyl::VinylSpin,

    /// 已确定的搜索页面封面端点。
    pub(super) search_flight: Option<flight::FlightPlan>,

    /// 已确定的全屏页面封面端点。
    pub(super) fullscreen_flight: Option<flight::FlightPlan>,

    /// 各浮层内容的独立输入。
    pub(super) overlay_inputs: crate::app::overlays::input::OverlayInputs<'a>,

    /// 已采样的氛围动画。
    pub(super) ambient: &'a ambient::AmbientGradient,

    /// 本拍响度包络。
    pub(super) ambient_pulse: &'a ambient::LoudnessPulse,

    /// 浮层的只读显示状态。
    pub(super) overlays: &'a crate::components::popup::OverlayStack<crate::app::AppOverlay>,

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
                    crate::view::lyrics::prepare(
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
                self.state
                    .transport
                    .prepare(full.transport, &self.state.playback, now);
            } else {
                let current = transform::morph_areas(
                    &normal,
                    &full,
                    self.state.browse.fullscreen.eased_in_out(),
                );
                let flight = crate::view::flight::plan_fullscreen(&normal, &full, &self.state);
                crate::view::browse::prepare(normal.left, &mut self.state, &self.theme, advance);
                if let Some(right) = normal.right {
                    crate::view::now_playing::prepare(right, &mut self.state, flight.is_some());
                }
                if let Some(plan) = flight {
                    let mut images = crate::image::ImageNeeds::new(self.state.images.ready());
                    flight::prepare(&plan, &mut images);
                    self.state.images.reconcile(images.finish());
                } else if let Some(cover) = current.cover {
                    prepare_fullscreen_cover(cover, full.cover, &mut self.state);
                }
                if let Some(lyric) = normal.lyrics {
                    crate::view::lyrics::prepare(
                        lyric,
                        &mut self.state,
                        &self.theme,
                        lyrics::LyricMode::Compact,
                        sample,
                    );
                }
                if let Some(lyric) = full.lyrics {
                    crate::view::lyrics::prepare(
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
                self.state
                    .transport
                    .prepare(current.transport, &self.state.playback, now);
            }
        } else if !self.state.channel_search.active.at_min() {
            let moving = !self.state.channel_search.active.at_max();
            let flight = moving
                .then(|| crate::view::flight::plan(&normal, &search, &self.state))
                .flatten();
            if moving {
                crate::view::browse::prepare(normal.left, &mut self.state, &self.theme, advance);
                if let Some(right) = normal.right {
                    crate::view::now_playing::prepare(right, &mut self.state, flight.is_some());
                }
                if let Some(lyric) = normal.lyrics {
                    crate::view::lyrics::prepare(
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
            crate::view::search::prepare(
                search.left,
                search.right,
                &mut self.state,
                &self.theme,
                flight.is_some(),
                advance,
            );
            if let Some(plan) = flight {
                let mut images = crate::image::ImageNeeds::new(self.state.images.ready());
                flight::prepare(&plan, &mut images);
                self.state.images.reconcile(images.finish());
            }
            let current = transform::morph_search(
                &normal,
                &search,
                self.state.channel_search.active.eased_in_out(),
            );
            self.state
                .transport
                .prepare(current.transport, &self.state.playback, now);
        } else {
            crate::view::browse::prepare(normal.left, &mut self.state, &self.theme, advance);
            if let Some(right) = normal.right {
                crate::view::now_playing::prepare(right, &mut self.state, false);
            }
            if let Some(lyric) = normal.lyrics {
                crate::view::lyrics::prepare(
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
            self.state
                .transport
                .prepare(normal.transport, &self.state.playback, now);
        }
        if self.transition.is_none() {
            self.overlays
                .prepare_content(area, &mut self.state, &self.theme, advance);
        }
        self.state.images.finish_preparation();
    }

    /// 只借出绘制所需的状态；该借用存续期间不能更新应用。
    pub(crate) fn frame_view(&self) -> FrameView<'_> {
        let state = &self.state;
        let theme = &self.theme;
        let normal = compute(state.frame_area, state.cfg.tui().layout());
        FrameView {
            env: crate::components::frame::FrameEnv {
                config: &state.cfg,
                theme,
                now: state.frame_now,
            },
            browse: super::browse::view(state, theme),
            selected: super::now_playing::view(state, theme),
            lyrics: super::lyrics::view(state, theme),
            search: super::search::view(state, theme),
            detail: super::search::detail_view(state, theme),
            transport: super::transport::view(state, theme),
            status: super::status::input(state),
            spectrum: &state.spectrum,
            fullscreen: &state.browse.fullscreen,
            ambient_reveal: &state.browse.ambient_reveal,
            images: state.images.ready(),
            playing: state.playback.track.as_ref(),
            cover_transition: state.images.transition.as_ref(),
            vinyl: &state.vinyl,
            search_flight: super::flight::plan(
                &normal,
                &compute_search(state.frame_area, state.cfg.tui().layout()),
                state,
            ),
            fullscreen_flight: super::flight::plan_fullscreen(
                &normal,
                &compute_fullscreen(state.frame_area, state.cfg.tui().layout()),
                state,
            ),
            overlay_inputs: crate::app::overlays::input::all(state),
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
