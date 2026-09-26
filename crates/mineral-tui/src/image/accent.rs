//! 封面色的消费协调:当前播放封面的色板就绪后,同时驱动频谱色场过渡、
//! waveform 的在播色板(`images.current_palette`)与动态 accent 渐变
//! (共用同一次封面身份 diff,过渡时长各自独立)。
//!
//! 身份判定(`cover_url` 变化、色带是否就绪)全在 app 层;频谱与 accent 状态机
//! 只收命令,不持有歌曲 / URL 身份。

use std::sync::Arc;

use crate::image::colors::derive_accents;

impl crate::app::App {
    /// 协调当前播放封面的配色消费:新封面取色就绪则从**当前可见配色**缓动过去,
    /// 否则保持现状。频谱走 `begin_cover_transition` / `clear_cover`,waveform 现读
    /// `images.current_palette`，动态 accent 走 `accent_fade.set_target`、全屏氛围背景
    /// 走 `ambient.set_target`(两者关闭时恒投 `None`)。
    ///
    /// - 当前封面与 `spectrum_cover` 一致 → 不动。
    /// - 当前封面变了 + 色带已就绪 → 频谱过渡 + accent / 氛围渐变到封面色,记下 key。
    /// - 当前封面变了 + 图已到但取色失败(在图片缓存却不在色板缓存)
    ///   → 频谱回退 hue、accent / 氛围渐变回 base,标记已处理。
    /// - 当前封面变了 + 图还在抓 → **保持当前可见态**(上一张封面继续显示),下个 tick 再看。
    ///   这是"红专辑换蓝专辑 → 红→蓝"的关键:抓图途中不回退,等蓝就绪直接红→蓝。
    /// - 无当前歌 / 无封面 → 频谱回 hue、accent / 氛围渐变回 base。
    pub(crate) fn sync_cover_palette(&mut self) {
        let cur = self
            .state
            .player
            .current
            .as_ref()
            .and_then(|s| s.cover_url.clone());
        let Some(url) = cur else {
            if self.state.images.spectrum_cover.is_some() {
                self.state.spectrum.clear_cover();
                self.accent_fade.set_target(/*to*/ None, &self.theme_base);
                self.feed_ambient(/*palette*/ None);
                self.state.images.spectrum_cover = None;
                self.state.images.current_palette = None;
            }
            return;
        };
        if self.state.images.spectrum_cover.as_ref() == Some(&url) {
            return;
        }
        if let Some(palette) = self.state.images.palettes.get(&url).cloned() {
            let accents = self
                .dynamic_accent_enabled()
                .then(|| derive_accents(&palette));
            self.accent_fade.set_target(accents, &self.theme_base);
            self.feed_ambient(Some(&palette));
            self.state
                .spectrum
                .begin_cover_transition(palette.clone(), &self.theme);
            self.state.images.spectrum_cover = Some(url);
            self.state.images.current_palette = Some(palette);
        } else if self.state.images.cache.contains_key(&url) {
            // 图已回但无色板 = 取色失败:回退,标记已处理(不再每帧重试)。
            self.state.spectrum.clear_cover();
            self.accent_fade.set_target(/*to*/ None, &self.theme_base);
            self.feed_ambient(/*palette*/ None);
            self.state.images.spectrum_cover = Some(url);
            self.state.images.current_palette = None;
        }
        // else:封面还在抓,保持当前可见态(上一张封面 / hue)不动,等就绪后再红→蓝。
    }

    /// 推进封面驱动的两台配色渐变一拍:动态 accent(并合成本帧 effective theme,
    /// 静止且无封面目标时合成是恒等)与全屏氛围背景(漂移速率 / 轮转周期现读配置)。
    pub(crate) fn tick_cover_fades(&mut self) {
        self.accent_fade.tick();
        self.theme = Arc::new(self.accent_fade.apply(self.theme_base));
        let ambient_cfg = self.state.cfg.tui().ambient();
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

    /// 向氛围渐变投喂目标:开关关闭 / 无色板 → 回落底色场。
    /// ANSI 主题无真彩底色,氛围场不可用,状态机保持现状(渲染侧同样拦下)。
    pub(crate) fn feed_ambient(&mut self, palette: Option<&crate::render::palette::CoverPalette>) {
        let Some(base) = crate::render::ambient::rgb_of(self.theme_base.base) else {
            return;
        };
        let ambient_cfg = self.state.cfg.tui().ambient();
        let enabled = *ambient_cfg.enabled();
        self.ambient
            .set_target(palette.filter(|_| enabled), base, ambient_cfg.anchors());
    }

    /// 动态主题开关(现读配置 `theme.dynamic.enabled`,热更下一次封面 diff 生效)。
    fn dynamic_accent_enabled(&self) -> bool {
        *self.state.cfg.tui().theme().dynamic().enabled()
    }
}
