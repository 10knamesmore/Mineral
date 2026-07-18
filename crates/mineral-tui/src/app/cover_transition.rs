//! 全屏切歌封面转场的触发协调:在播封面身份 diff 出切歌瞬间,新旧图都就绪则开一段
//! halfblock 合成转场;渲染处只读 `covers.transition`,推进 / 收尾都在这里。
//!
//! 触发刻意保守:仅全屏稳态(形变 / 滚动各有自己的 halfblock 路径)、且 from / to
//! 两图都已在缓存——缺任一图维持现状行为(无图直接换 / 编码在途 halfblock),不强行转场。

use mineral_model::MediaUrl;

use crate::render::anim::{Transition, ticks16_from_ms};
use crate::runtime::state::CoverTransition;

impl crate::app::App {
    /// 每帧同步全屏切歌封面转场:推进在途转场、推满即收尾回稳态渲染;全屏稳态下
    /// 在播封面身份变化且两图都命中缓存时开新转场(切歌打断旧转场,直接换段)。
    /// 非全屏稳态只跟随显示身份、并取消在途转场。
    pub(crate) fn sync_cover_transition(&mut self) {
        if let Some(active) = self.state.covers.transition.as_mut() {
            active.anim.tick();
            if active.anim.at_max() {
                self.state.covers.transition = None;
            }
        }
        let cur = self
            .state
            .playback
            .track
            .as_ref()
            .and_then(|s| s.cover_url.clone());
        if !self.state.browse.fullscreen.at_max() {
            self.state.covers.displayed_cover = cur;
            self.state.covers.transition = None;
            return;
        }
        if self.state.covers.displayed_cover == cur {
            return;
        }
        let prev = std::mem::replace(&mut self.state.covers.displayed_cover, cur.clone());
        let transition_cfg = self.state.cfg.tui().cover_transition();
        if !*transition_cfg.enabled() {
            return;
        }
        let duration_ms = *transition_cfg.duration_ms();
        let tick_ms = *self.state.cfg.tui().animation().frame_tick_ms();
        let (Some(from_url), Some(to_url)) = (prev, cur) else {
            return;
        };
        let cached = |url: &MediaUrl| self.state.covers.cache.contains_key(url);
        if !(cached(&from_url) && cached(&to_url)) {
            return;
        }
        self.state.covers.transition = Some(CoverTransition {
            from_url,
            to_url,
            anim: Transition::expanding(ticks16_from_ms(duration_ms, tick_ms)),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mineral_model::MediaUrl;

    use crate::render::anim::Toggle;
    use crate::test_support::app_with_queue;

    /// 造「全屏稳态 + 在播曲带封面 A(已缓存)+ 已同步过一次」的 App。
    fn steady_fullscreen_showing(url: &MediaUrl) -> color_eyre::Result<crate::app::App> {
        let mut app = app_with_queue(/*len*/ 2, /*current_idx*/ 0)?;
        if let Some(song) = app.state.player.queue.first() {
            let mut track = song.clone();
            track.cover_url = Some(url.clone());
            app.state.playback.track = Some(track);
        }
        cache_image(&mut app, url);
        let mut fs = Toggle::new(1);
        fs.set(true);
        fs.tick();
        app.state.browse.fullscreen = fs;
        app.sync_cover_transition();
        assert_eq!(
            app.state.covers.displayed_cover.as_ref(),
            Some(url),
            "前置:显示身份已跟上"
        );
        assert!(app.state.covers.transition.is_none(), "前置:首帧不转场");
        Ok(app)
    }

    /// 往缓存塞一张该 URL 的解码图。
    fn cache_image(app: &mut crate::app::App, url: &MediaUrl) {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(16, 16));
        app.state.covers.cache.insert(url, Arc::new(img));
    }

    /// 把在播曲封面换成 `url`(模拟切歌后的 playback 镜像)。
    fn switch_track_cover(app: &mut crate::app::App, url: &MediaUrl) {
        if let Some(track) = app.state.playback.track.as_mut() {
            track.cover_url = Some(url.clone());
        }
    }

    /// 触发链:全屏稳态 + 封面 A→B 且两图都在缓存 → 开转场,from/to 身份正确。
    #[test]
    fn switch_with_both_cached_starts_transition() -> color_eyre::Result<()> {
        let a = MediaUrl::remote("https://x.y/a.jpg")?;
        let b = MediaUrl::remote("https://x.y/b.jpg")?;
        let mut app = steady_fullscreen_showing(&a)?;
        cache_image(&mut app, &b);
        switch_track_cover(&mut app, &b);
        app.sync_cover_transition();
        let t = app
            .state
            .covers
            .transition
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("应开启转场"))?;
        assert_eq!(t.from_url, a, "from 应是切歌前显示的图");
        assert_eq!(t.to_url, b, "to 应是新在播图");
        Ok(())
    }

    /// 新图未入缓存:不转场(维持现状行为),但显示身份已跟上——图迟到后**不再**补转场。
    #[test]
    fn switch_with_missing_image_does_not_transition() -> color_eyre::Result<()> {
        let a = MediaUrl::remote("https://x.y/a.jpg")?;
        let b = MediaUrl::remote("https://x.y/b.jpg")?;
        let mut app = steady_fullscreen_showing(&a)?;
        switch_track_cover(&mut app, &b); // b 故意不入缓存
        app.sync_cover_transition();
        assert!(app.state.covers.transition.is_none(), "缺图不强行转场");
        assert_eq!(
            app.state.covers.displayed_cover.as_ref(),
            Some(&b),
            "显示身份仍应跟上"
        );
        cache_image(&mut app, &b);
        app.sync_cover_transition();
        assert!(
            app.state.covers.transition.is_none(),
            "图迟到不补转场(身份已 diff 过)"
        );
        Ok(())
    }

    /// 非全屏稳态:切歌只跟随显示身份,不开转场。
    #[test]
    fn switch_outside_fullscreen_does_not_transition() -> color_eyre::Result<()> {
        let a = MediaUrl::remote("https://x.y/a.jpg")?;
        let b = MediaUrl::remote("https://x.y/b.jpg")?;
        let mut app = steady_fullscreen_showing(&a)?;
        app.state.browse.fullscreen = Toggle::new(1); // 回浏览态
        cache_image(&mut app, &b);
        switch_track_cover(&mut app, &b);
        app.sync_cover_transition();
        assert!(app.state.covers.transition.is_none(), "非全屏不转场");
        assert_eq!(
            app.state.covers.displayed_cover.as_ref(),
            Some(&b),
            "显示身份仍应跟上"
        );
        Ok(())
    }

    /// 转场推满即收尾:transition 清空,回稳态渲染路径。
    #[test]
    fn transition_clears_after_completion() -> color_eyre::Result<()> {
        let a = MediaUrl::remote("https://x.y/a.jpg")?;
        let b = MediaUrl::remote("https://x.y/b.jpg")?;
        let mut app = steady_fullscreen_showing(&a)?;
        cache_image(&mut app, &b);
        switch_track_cover(&mut app, &b);
        app.sync_cover_transition();
        assert!(app.state.covers.transition.is_some(), "前置:转场已开");
        // 默认 900ms / 16ms ≈ 57 拍,推 200 帧余量收尾。
        for _ in 0..200 {
            app.sync_cover_transition();
        }
        assert!(app.state.covers.transition.is_none(), "推满应收尾清空");
        Ok(())
    }

    /// `cover_transition.enabled = false`:切歌直接换,不开转场。
    #[test]
    fn disabled_config_skips_transition() -> color_eyre::Result<()> {
        let a = MediaUrl::remote("https://x.y/a.jpg")?;
        let b = MediaUrl::remote("https://x.y/b.jpg")?;
        let mut app = steady_fullscreen_showing(&a)?;
        app.apply_pushed_config(mineral_protocol::BusValue::from_json(
            mineral_config::merge_tree(
                mineral_config::default_tree()?,
                serde_json::json!({ "tui": { "cover_transition": { "enabled": false } } }),
            ),
        ));
        cache_image(&mut app, &b);
        switch_track_cover(&mut app, &b);
        app.sync_cover_transition();
        assert!(app.state.covers.transition.is_none(), "关闭后不应转场");
        Ok(())
    }
}
