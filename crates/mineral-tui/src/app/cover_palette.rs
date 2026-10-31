//! 将当前播放封面的色板应用到频谱、动态主题与氛围背景。

use std::sync::Arc;

use crate::app::App;
use crate::image::colors::derive_accents;
use crate::render::ambient::rgb_of;
use crate::render::palette::CoverPalette;
use crate::runtime::state::AppliedCoverPalette;

impl App {
    /// 新封面完成解码后更新配色目标；加载期间保留上一张封面的可见颜色。
    pub(crate) fn sync_cover_palette(&mut self) {
        let current = self
            .state
            .models
            .player
            .current
            .as_ref()
            .and_then(|song| song.cover_url.clone());
        let Some(url) = current else {
            if self.state.ui.cover.applied_palette.take().is_some() {
                self.state.ui.spectrum.clear_cover();
                self.accent_fade.set_target(None, &self.theme_base);
                self.feed_ambient(None);
            }
            return;
        };
        if self
            .state
            .ui
            .cover
            .applied_palette
            .as_ref()
            .is_some_and(|applied| applied.url == url)
        {
            return;
        }
        if !self.state.resources.images.cache.contains_key(&url) {
            return;
        }
        let palette = self.state.resources.images.cache.palette(&url).cloned();
        if let Some(palette) = &palette {
            let dynamic = self.state.cfg.theme().dynamic();
            let accents = (*dynamic.enabled()).then(|| {
                derive_accents(
                    palette,
                    self.theme_base.background_rgb(),
                    *dynamic.minimum_contrast(),
                )
            });
            self.accent_fade.set_target(accents, &self.theme_base);
            self.feed_ambient(Some(palette));
            self.state
                .ui
                .spectrum
                .begin_cover_transition(palette.clone(), &self.theme);
        } else {
            self.state.ui.spectrum.clear_cover();
            self.accent_fade.set_target(None, &self.theme_base);
            self.feed_ambient(None);
        }
        self.state.ui.cover.applied_palette = Some(AppliedCoverPalette { url, palette });
    }

    /// 推进动态强调色与全屏氛围的渐变，更新本帧使用的主题。
    pub(crate) fn tick_cover_fades(&mut self) {
        self.accent_fade.tick();
        self.theme = Arc::new(self.accent_fade.apply(self.theme_base));
        let ambient_cfg = self.state.cfg.ambient();
        let drift = ambient_cfg.drift();
        let drift_speed = if *drift.enabled() {
            *drift.speed()
        } else {
            0.0
        };
        let rotate = ambient_cfg.rotate();
        let rotate_cycle = if *rotate.enabled() {
            *rotate.cycle_secs()
        } else {
            0.0
        };
        self.ambient.tick(drift_speed, rotate_cycle);
    }

    /// 将封面配色交给氛围渐变；功能关闭时回到底色，非 RGB 底色不参与计算。
    pub(crate) fn feed_ambient(&mut self, palette: Option<&CoverPalette>) {
        let Some(base) = rgb_of(self.theme_base.base) else {
            return;
        };
        let config = self.state.cfg.ambient();
        self.ambient.set_target(
            palette.filter(|_| *config.enabled()),
            base,
            config.anchors(),
        );
    }
}
