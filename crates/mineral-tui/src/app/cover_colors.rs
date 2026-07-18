//! 封面色的消费协调:当前播放封面的色板就绪后,同时驱动频谱色场过渡、
//! waveform 的在播色板(`covers.current_palette`)与动态 accent 渐变
//! (共用同一次封面身份 diff,过渡时长各自独立)。
//!
//! 身份判定(`cover_url` 变化、色带是否就绪)全在 app 层;频谱与 accent 状态机
//! 只收命令,不持有歌曲 / URL 身份。

use std::sync::Arc;

use crate::runtime::cover::colors::derive_accents;

impl crate::app::App {
    /// 协调当前播放封面的配色消费:新封面取色就绪则从**当前可见配色**缓动过去,
    /// 否则保持现状。频谱走 `begin_cover_transition` / `clear_cover`,waveform 现读
    /// `covers.current_palette`,动态 accent 走 `accent_fade.set_target`、全屏氛围背景
    /// 走 `ambient.set_target`(两者关闭时恒投 `None`)。
    ///
    /// - 当前封面与 `spectrum_cover` 一致 → 不动。
    /// - 当前封面变了 + 色带已就绪 → 频谱过渡 + accent / 氛围渐变到封面色,记下 key。
    /// - 当前封面变了 + 图已到但**取色失败**(在 `covers.cache` 却不在 `covers.palettes`)
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
            if self.state.covers.spectrum_cover.is_some() {
                self.state.spectrum.clear_cover();
                self.accent_fade.set_target(/*to*/ None, &self.theme_base);
                self.feed_ambient(/*palette*/ None);
                self.state.covers.spectrum_cover = None;
                self.state.covers.current_palette = None;
            }
            return;
        };
        if self.state.covers.spectrum_cover.as_ref() == Some(&url) {
            return;
        }
        if let Some(palette) = self.state.covers.palettes.get(&url).cloned() {
            let accents = self
                .dynamic_accent_enabled()
                .then(|| derive_accents(&palette));
            self.accent_fade.set_target(accents, &self.theme_base);
            self.feed_ambient(Some(&palette));
            self.state
                .spectrum
                .begin_cover_transition(palette.clone(), &self.theme);
            self.state.covers.spectrum_cover = Some(url);
            self.state.covers.current_palette = Some(palette);
        } else if self.state.covers.cache.contains_key(&url) {
            // 图已回但无色板 = 取色失败:回退,标记已处理(不再每帧重试)。
            self.state.spectrum.clear_cover();
            self.accent_fade.set_target(/*to*/ None, &self.theme_base);
            self.feed_ambient(/*palette*/ None);
            self.state.covers.spectrum_cover = Some(url);
            self.state.covers.current_palette = None;
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

#[cfg(test)]
mod tests {
    use mineral_model::MediaUrl;
    use ratatui::style::Color;

    use crate::render::palette::{CoverPalette, Rgb};
    use crate::runtime::cover::colors::derive_accents;
    use crate::test_support::app_with_queue;

    /// 造一个非空测试色板。
    fn palette(swatches: Vec<Rgb>) -> color_eyre::Result<CoverPalette> {
        CoverPalette::new(swatches).ok_or_else(|| color_eyre::eyre::eyre!("非空色板"))
    }

    /// 回归(红→蓝路径之二):换歌后新封面**还在抓取**时,频谱保持上一张封面,
    /// **不**回退到 hue——这样等新色板就绪能直接红→蓝,而非 hue→蓝。
    #[test]
    fn sync_holds_previous_cover_until_new_palette_ready() -> color_eyre::Result<()> {
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        let red = MediaUrl::remote("https://example.com/red.jpg")?;
        let blue = MediaUrl::remote("https://example.com/blue.jpg")?;
        // 当前在播这首歌封面是 blue,但频谱上一张应用的是 red。
        if let Some(song) = app.state.player.current.as_mut() {
            song.cover_url = Some(blue);
        }
        app.state.covers.spectrum_cover = Some(red.clone());
        // blue 的色板 / 图都还没到 —— sync 应原地保持(不清、不抢先标记)。
        app.sync_cover_palette();
        assert_eq!(
            app.state.covers.spectrum_cover.as_ref(),
            Some(&red),
            "抓图途中应保持上一张封面"
        );
        Ok(())
    }

    /// 回归(红→蓝路径之一):新封面色板就绪即触发过渡并记下其 key。
    #[test]
    fn sync_begins_transition_when_palette_ready() -> color_eyre::Result<()> {
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        let blue = MediaUrl::remote("https://example.com/blue.jpg")?;
        if let Some(song) = app.state.player.current.as_mut() {
            song.cover_url = Some(blue.clone());
        }
        let pal = palette(vec![Rgb::new(20, 20, 120), Rgb::new(40, 40, 200)])?;
        app.state.covers.palettes.insert(blue.clone(), pal);
        app.sync_cover_palette();
        assert_eq!(
            app.state.covers.spectrum_cover.as_ref(),
            Some(&blue),
            "色板就绪应记下并触发过渡"
        );
        assert!(
            app.state.covers.current_palette.is_some(),
            "waveform 的在播色板应同步建立"
        );
        Ok(())
    }

    /// 动态 accent 主链路:色板就绪 → sync 投目标 → 渐变推满后 effective theme 的
    /// accent 对 = 封面派生色;base 主题不动。
    #[test]
    fn dynamic_accent_follows_cover_palette() -> color_eyre::Result<()> {
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        let base_accent = app.theme_base.accent;
        let blue = MediaUrl::remote("https://example.com/blue.jpg")?;
        if let Some(song) = app.state.player.current.as_mut() {
            song.cover_url = Some(blue.clone());
        }
        let pal = palette(vec![Rgb::new(20, 20, 120), Rgb::new(40, 40, 200)])?;
        app.state.covers.palettes.insert(blue, pal.clone());
        app.sync_cover_palette();
        // 默认 fade_ms 3000 / tick 16ms ≈ 188 拍,推 400 拍余量到程。
        for _ in 0..400 {
            app.tick_cover_fades();
        }
        let expected = derive_accents(&pal);
        assert_eq!(
            app.theme.accent,
            Color::Rgb(expected.accent.r, expected.accent.g, expected.accent.b),
            "effective accent 应到达封面派生色"
        );
        assert_eq!(
            app.theme.accent_2,
            Color::Rgb(
                expected.accent_2.r,
                expected.accent_2.g,
                expected.accent_2.b
            ),
            "effective accent_2 应到达封面派生副色"
        );
        assert_ne!(app.theme.accent, base_accent, "应离开 base accent");
        assert_eq!(app.theme_base.accent, base_accent, "base 主题不被改写");
        Ok(())
    }

    /// 渐变是渐进的:sync 后推少量拍,accent 既不在 base 也不在终点(中间色)。
    #[test]
    fn dynamic_accent_fades_gradually() -> color_eyre::Result<()> {
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        let base_accent = app.theme_base.accent;
        let blue = MediaUrl::remote("https://example.com/blue.jpg")?;
        if let Some(song) = app.state.player.current.as_mut() {
            song.cover_url = Some(blue.clone());
        }
        let pal = palette(vec![Rgb::new(20, 20, 120), Rgb::new(40, 40, 200)])?;
        app.state.covers.palettes.insert(blue, pal.clone());
        app.sync_cover_palette();
        for _ in 0..20 {
            app.tick_cover_fades();
        }
        let mid = app.theme.accent;
        let expected = derive_accents(&pal);
        assert_ne!(mid, base_accent, "推进后应离开起点");
        assert_ne!(
            mid,
            Color::Rgb(expected.accent.r, expected.accent.g, expected.accent.b),
            "少量拍后不应已到终点(3s 渐变)"
        );
        Ok(())
    }

    /// 无封面(切到无 cover 的歌):accent 渐变回 base 静态 token。
    #[test]
    fn dynamic_accent_falls_back_without_cover() -> color_eyre::Result<()> {
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        let base_accent = app.theme_base.accent;
        let blue = MediaUrl::remote("https://example.com/blue.jpg")?;
        if let Some(song) = app.state.player.current.as_mut() {
            song.cover_url = Some(blue.clone());
        }
        let pal = palette(vec![Rgb::new(20, 20, 120), Rgb::new(40, 40, 200)])?;
        app.state.covers.palettes.insert(blue, pal);
        app.sync_cover_palette();
        for _ in 0..400 {
            app.tick_cover_fades();
        }
        assert_ne!(app.theme.accent, base_accent, "前置:已染上封面色");
        // 当前歌变成无封面。
        if let Some(song) = app.state.player.current.as_mut() {
            song.cover_url = None;
        }
        app.sync_cover_palette();
        for _ in 0..400 {
            app.tick_cover_fades();
        }
        assert_eq!(app.theme.accent, base_accent, "无封面应渐变回 base accent");
        assert!(
            app.state.covers.current_palette.is_none(),
            "无封面时 waveform 的在播色板应清空"
        );
        Ok(())
    }
}
