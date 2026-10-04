//! 页面自身绘制的层：根背景与搜索页组合沿用已有组件身份。

use ratatui::{Frame, layout::Rect};

use super::preparation::FrameView;
use crate::components::frame::FrameEnv;
use crate::components::layout::shared::compute::Areas;
use crate::components::lifecycle::PaintView;
use crate::render::memo::Dependencies;

/// 根组件负责的背景、顶栏和全屏封面；子组件独立登记。
pub(super) struct RootLayer<'a> {
    /// 本帧只读应用视图。
    pub(super) app: &'a FrameView<'a>,

    /// 常规布局，用于背景避让。
    pub(super) normal: Areas,

    /// 当前稳态页面的布局。
    pub(super) areas: Areas,

    /// 是否显示全屏封面。
    pub(super) full: bool,

    /// 是否显示搜索页面。
    pub(super) search: bool,
}

impl PaintView for RootLayer<'_> {
    fn dependencies(&self, _area: Rect, env: FrameEnv<'_>, inputs: &mut Dependencies<'_>) {
        let app = self.app;
        inputs.observe(&(self.full, self.search));
        inputs.observe(&app.ambient_reveal.progress());
        let ambient_cfg = env.config.tui().ambient();
        if app.ambient_reveal.active() && (*ambient_cfg.enabled() || !app.ambient.settled_at_base())
        {
            inputs.observe(app.ambient);
            if *ambient_cfg.pulse().enabled() {
                inputs.observe(&app.ambient_pulse.level_permille(ambient_cfg.pulse()));
            }
        }
        if self.full {
            inputs.optional(app.playing);
            inputs.optional(app.cover_transition);
            app.images.dependencies(
                inputs,
                [
                    app.playing.and_then(|song| song.cover_url.clone()),
                    app.cover_transition
                        .map(|transition| transition.from_url.clone()),
                    app.cover_transition
                        .map(|transition| transition.to_url.clone()),
                ],
            );
            if app.playing.is_none() {
                inputs.time(env.now);
            }
        } else if !self.search {
            app.status.dependencies(inputs);
            app.images.dependencies(
                inputs,
                [
                    app.selected.input.playlist_cover.clone(),
                    app.selected
                        .input
                        .track
                        .and_then(|entry| entry.data.song.cover_url.clone()),
                ],
            );
        }
    }

    fn paint(&self, frame: &mut Frame<'_>, _area: Rect, env: FrameEnv<'_>) {
        super::frame::paint_backdrop(frame, self.app, &self.normal, env.config.tui().layout());
        if self.full {
            if let Some(cover) = self.areas.cover.filter(|area| !area.is_empty()) {
                super::frame::draw_fullscreen_cover(frame, cover, self.app);
            }
        } else if !self.search {
            crate::components::layout::shared::top_status::draw(
                frame,
                self.areas.top_status,
                &self.app.status,
                env.theme,
            );
        }
    }
}

/// 搜索页沿用一个组合单元，结果、输入框与详情共享同一准备布局。
pub(super) struct SearchLayer<'a> {
    /// 本帧搜索及详情的只读输入。
    pub(super) app: &'a FrameView<'a>,

    /// 搜索页已选定的布局。
    pub(super) areas: Areas,
}

impl PaintView for SearchLayer<'_> {
    fn dependencies(&self, _area: Rect, _env: FrameEnv<'_>, inputs: &mut Dependencies<'_>) {
        self.app
            .search
            .dependencies(&self.areas, &self.app.detail, inputs);
    }

    fn paint(&self, frame: &mut Frame<'_>, _area: Rect, _env: FrameEnv<'_>) {
        super::frame::paint_search(frame, &self.areas, self.app, false);
    }
}
