//! 验证全屏切图转场的应用接线。

#[cfg(test)]
impl crate::app::App {
    /// 按当前应用状态推进图片引擎中的全屏切图转场。
    fn sync_cover_transition(&mut self) {
        let current_cover = self
            .state
            .playback
            .track
            .as_ref()
            .and_then(|track| track.cover_url.clone());
        self.state
            .images
            .sync_transition(current_cover, self.state.browse.fullscreen.at_max());
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
            app.state.images.displayed_cover.as_ref(),
            Some(url),
            "前置:显示身份已跟上"
        );
        assert!(app.state.images.transition.is_none(), "前置:首帧不转场");
        Ok(app)
    }

    /// 往缓存塞一张该 URL 的解码图。
    fn cache_image(app: &mut crate::app::App, url: &MediaUrl) {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(16, 16));
        app.state.images.cache.insert(url, Arc::new(img));
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
            .images
            .transition
            .as_ref()
            .ok_or_else(|| color_eyre::eyre::eyre!("应开启转场"))?;
        assert_eq!(t.from_url, a, "from 应是切歌前显示的图");
        assert_eq!(t.to_url, b, "to 应是新在播图");
        Ok(())
    }

    /// 新图未入缓存时不启动转场，但立即更新显示身份；图片迟到后也不补启动转场。
    #[test]
    fn switch_with_missing_image_does_not_transition() -> color_eyre::Result<()> {
        let a = MediaUrl::remote("https://x.y/a.jpg")?;
        let b = MediaUrl::remote("https://x.y/b.jpg")?;
        let mut app = steady_fullscreen_showing(&a)?;
        switch_track_cover(&mut app, &b); // b 故意不入缓存
        app.sync_cover_transition();
        assert!(app.state.images.transition.is_none(), "缺图不强行转场");
        assert_eq!(
            app.state.images.displayed_cover.as_ref(),
            Some(&b),
            "显示身份仍应跟上"
        );
        cache_image(&mut app, &b);
        app.sync_cover_transition();
        assert!(
            app.state.images.transition.is_none(),
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
        assert!(app.state.images.transition.is_none(), "非全屏不转场");
        assert_eq!(
            app.state.images.displayed_cover.as_ref(),
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
        assert!(app.state.images.transition.is_some(), "前置:转场已开");
        // 默认 900ms / 16ms ≈ 57 拍,推 200 帧余量收尾。
        for _ in 0..200 {
            app.sync_cover_transition();
        }
        assert!(app.state.images.transition.is_none(), "推满应收尾清空");
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
        assert!(app.state.images.transition.is_none(), "关闭后不应转场");
        Ok(())
    }
}
