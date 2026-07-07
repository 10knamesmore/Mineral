//! 有效配置的应用单入口:daemon 托管配置,client 不看文件。
//!
//! daemon 是唯一 watcher / 合成者(文件重载 + 脚本覆盖都在 daemon 侧合成),
//! client 经 `Event::ConfigChanged` 收有效配置树,落型后走 [`App::apply_config`]
//! 应用——与启动自举同一套 from_config / retempo,不存在第二条应用路径。
//!
//! 应用语义分两类:**现读型**字段(布局 / 行距 / 步长等)随 `state.cfg` 换 Arc
//! 下一帧天然生效;**固化型**(拍数折算 / FFT 预计算 / 缓存预算)在这里统一
//! 就地重设——保留运行态(动画相位 / 存活通知 / 已缓存封面),只换参数。
//! 新增固化型配置消费点必须挂进 [`App::apply_config`] 并配重载测试。

use std::sync::Arc;

use crate::runtime::marquee::Marquees;

/// daemon 推送的配置落型失败时的驻留错误卡顶替键(下一帧好配置到来主动撤)。
const PUSH_CARD_ID: &str = "config.push";

impl crate::app::App {
    /// 应用一份新的有效配置:换 `state.cfg` Arc + 全部固化型消费点就地重设。
    ///
    /// 启动自举(`App::new` 的构造参数)与 daemon 推送(`Event::ConfigChanged`)
    /// 共用此入口的 from_config / retempo 集合;运行态(动画相位 / 存活通知 /
    /// 封面缓存 / 搜索会话)一律保留。
    ///
    /// # Params:
    ///   - `cfg`: 新有效配置(`Arc` 共享只读)
    pub(crate) fn apply_config(&mut self, cfg: Arc<mineral_config::Config>) {
        self.state.cfg = cfg;
        let cfg = Arc::clone(&self.state.cfg);
        let tui_cfg = cfg.tui();
        let anim = tui_cfg.animation();
        let tick_ms = *anim.frame_tick_ms();
        // 固化型(重建即换,无运行态):主题色 token / 窗口标题模板 / keymap 查表。
        self.theme = Arc::new(crate::render::theme::Theme::from_config(tui_cfg.theme()));
        self.window_title = crate::runtime::window_title::WindowTitle::new(tui_cfg.window_title());
        self.rebuild_keymap();
        // 固化型(带槽相位,整体重建 + 相位 reconciliation 在其内部):marquee / 唱片纹。
        self.state.marquees = Marquees::from_config(anim.marquee(), tick_ms);
        self.state.vinyl = crate::components::layout::shared::vinyl::VinylSpin::from_config(
            *anim.vinyl_rev_ms(),
            tick_ms,
        );
        // 固化型(携带运行态,就地重设保相位 / 保存活项 / 保缓存)。
        self.overlays.retempo(crate::render::anim::ticks16_from_ms(
            *anim.popup_anim_ms(),
            tick_ms,
        ));
        self.notifications.retempo(
            *tui_cfg.toast().flash_ttl_secs(),
            crate::render::anim::ticks16_from_ms(*anim.toast_anim_ms(), tick_ms),
        );
        self.state.browse.retempo(anim);
        self.state.channel_search.reconfigure(
            crate::render::anim::ticks16_from_ms(*anim.fullscreen_ms(), tick_ms),
            crate::render::anim::ticks16_from_ms(*anim.search_focus_morph_ms(), tick_ms),
            crate::runtime::state::search_whitelist::SearchWhitelist::from(
                tui_cfg.search().channel(),
            ),
        );
        self.state.dim.retempo(crate::render::anim::ticks16_from_ms(
            *anim.focus_fade_ms(),
            tick_ms,
        ));
        self.state
            .spectrum
            .reconfigure(tui_cfg.spectrum().clone(), tick_ms);
        // FFT 预计算贵且重建丢样本环缓冲(频谱空一两帧),参数没变不动。
        let params = crate::runtime::state::spectrum_params(tui_cfg.spectrum());
        if self.state.fft.params() != &params {
            self.state.fft = mineral_spectrum::SpectrumComputer::new(params);
        }
        self.state.covers.set_budgets(
            *tui_cfg.cover().cache().image(),
            *tui_cfg.cover().cache().protocol(),
        );
    }

    /// 消费一帧 daemon 推送的有效配置树:落型成功即应用;失败(版本偏斜等,
    /// daemon 侧已校验、正常不该发生)保留现行配置 + 驻留错误卡。
    ///
    /// 握手重放的第一帧静默应用;之后的变更 flash 提示。
    ///
    /// # Params:
    ///   - `config`: 有效配置树(wire 形)
    pub(crate) fn apply_pushed_config(&mut self, config: mineral_protocol::BusValue) {
        use crate::components::toast::card::{plain_body, plain_line};
        use crate::components::toast::notifications::{TextTint, tinted_text_item};
        match mineral_config::from_tree(&config.into_json()) {
            Ok(cfg) => {
                self.apply_config(Arc::new(cfg));
                self.notifications.dismiss_card_by_id(PUSH_CARD_ID);
                if self.config_replayed {
                    self.notifications
                        .flash(tinted_text_item("配置已更新".to_owned(), TextTint::Normal));
                }
                self.config_replayed = true;
            }
            Err(warning) => {
                mineral_log::warn!(
                    target: "tui",
                    warning = %warning,
                    "daemon 推送的配置落型失败,保留现行配置"
                );
                self.notifications.push_card(
                    TextTint::Error,
                    plain_line("config push rejected"),
                    plain_body(vec![
                        warning.to_string(),
                        "keeping current config".to_owned(),
                    ]),
                    Some(PUSH_CARD_ID.to_owned()),
                    /*ttl*/ None,
                );
            }
        }
    }

    /// 启动期配置提示(`run` 在 `App::new` 后调一次):
    ///   - `config_path` 不存在 → 驻留卡提醒 `mineral config init`(每次启动都提醒,
    ///     直到用户真的生成配置);
    ///   - 启动自举加载的降级告警 → 驻留警告卡(daemon 侧重载后的告警走推送通道)。
    ///
    /// # Params:
    ///   - `config_path`: config.lua 路径(解析失败给 `None`,跳过缺失检查)
    ///   - `warnings`: 启动加载产生的降级告警
    pub(crate) fn notify_startup_config(
        &mut self,
        config_path: Option<&std::path::Path>,
        warnings: &[mineral_config::ConfigWarning],
    ) {
        use crate::components::toast::card::{plain_body, plain_line};
        use crate::components::toast::notifications::TextTint;
        if let Some(path) = config_path
            && !path.exists()
        {
            self.notifications.push_card(
                TextTint::Normal,
                plain_line("config not found"),
                plain_body(vec![
                    "run `mineral config init` to create one".to_owned(),
                    format!("path: {}", path.display()),
                ]),
                Some("config.init".to_owned()),
                /*ttl*/ None,
            );
        }
        if !warnings.is_empty() {
            let lines = warnings.iter().map(ToString::to_string);
            self.notifications.push_card(
                TextTint::Warn,
                plain_line("config.lua warnings"),
                plain_body(lines),
                Some("config.reload".to_owned()),
                /*ttl*/ None,
            );
        }
    }

    /// daemon 推送 `ScriptReloaded` 后刷新脚本 bind 键(配置部分不动,
    /// 用现行 `state.cfg` 重建 keymap 再合新 bind 表)。
    pub(crate) fn refresh_script_binds(&mut self) {
        self.rebuild_keymap();
    }

    /// 以现行配置重建 keymap 并合入 daemon 的 bind 表;卡片关闭键提示随表刷新。
    fn rebuild_keymap(&mut self) {
        let tui_cfg = self.state.cfg.tui();
        let mut keymap =
            crate::runtime::keymap::Keymap::from_config(tui_cfg.keys(), tui_cfg.behavior());
        keymap.append_script_binds(&self.client.script_binds());
        self.notice_hint = Self::compose_notice_hint(&keymap);
        self.keymap = keymap;
    }
}

#[cfg(test)]
mod tests {
    use mineral_config::keys::KeyChord;
    use mineral_protocol::BusValue;

    use crate::runtime::action::Action;
    use crate::test_support::app_with_queue;

    /// 造一帧「default.lua + overlay」合成的有效配置树(wire 形),
    /// 与 daemon 侧合成路径同构。
    fn pushed_tree(overlay: serde_json::Value) -> color_eyre::Result<BusValue> {
        Ok(BusValue::from_json(mineral_config::merge_tree(
            mineral_config::default_tree()?,
            overlay,
        )))
    }

    /// 推送应用:keymap / theme 热生效;首帧(握手重放)静默、次帧 flash 提示。
    #[test]
    fn pushed_config_applies_keymap_and_theme() -> color_eyre::Result<()> {
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        let old_accent = app.theme.accent;
        assert_eq!(
            app.keymap.lookup(KeyChord::parse("<Space>")?),
            Some(Action::TogglePlayPause),
            "应用前默认绑定"
        );
        let entries_before = app.notifications.entry_count();
        app.apply_pushed_config(pushed_tree(serde_json::json!({ "tui": {
            "keys": { "play_pause": "w" },
            "theme": { "accent": "#ff0000" },
        } }))?);
        assert_eq!(
            app.keymap.lookup(KeyChord::parse("w")?),
            Some(Action::TogglePlayPause),
            "新键生效"
        );
        assert_eq!(
            app.keymap.lookup(KeyChord::parse("<Space>")?),
            None,
            "旧键整体替换"
        );
        assert_ne!(app.theme.accent, old_accent, "主题热应用");
        assert_eq!(
            app.notifications.entry_count(),
            entries_before,
            "首帧(握手重放)应静默"
        );
        app.apply_pushed_config(pushed_tree(
            serde_json::json!({ "audio": { "volume": 42 } }),
        )?);
        assert!(
            app.notifications.entry_count() > entries_before,
            "后续变更应 flash 提示"
        );
        Ok(())
    }

    /// marquee 节奏随推送热更:mode 改 off 后溢出标题恒零相位
    /// (证明 Marquees 用新配置重建了,而非沿用启动折算的快照)。
    #[test]
    fn pushed_config_rebuilds_marquee_tempo() -> color_eyre::Result<()> {
        use crate::runtime::marquee::Slot;

        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        // 默认 loop:溢出 + 推进足量后相位应非零(默认 pause 100ms/16ms ≈ 7 拍)。
        for _ in 0..200 {
            app.state.marquees.tick();
        }
        let scrolled = app
            .state
            .marquees
            .phase(
                Slot::Transport,
                "a",
                /*content_w*/ 40,
                /*window_w*/ 10,
                /*gap_w*/ 2,
            )
            .offset;
        // 建档在首查,再推进一轮令相位走起来。
        for _ in 0..200 {
            app.state.marquees.tick();
        }
        let scrolled = scrolled.max(
            app.state
                .marquees
                .phase(
                    Slot::Transport,
                    "a",
                    /*content_w*/ 40,
                    /*window_w*/ 10,
                    /*gap_w*/ 2,
                )
                .offset,
        );
        assert!(scrolled > 0, "应用前默认 loop 应在滚动");
        app.apply_pushed_config(pushed_tree(
            serde_json::json!({ "tui": { "animation": { "marquee": { "mode": "off" } } } }),
        )?);
        for _ in 0..200 {
            app.state.marquees.tick();
        }
        let after = app
            .state
            .marquees
            .phase(
                Slot::Transport,
                "a",
                /*content_w*/ 40,
                /*window_w*/ 10,
                /*gap_w*/ 2,
            )
            .offset;
        assert_eq!(after, 0, "热更为 off 后应恒零相位");
        Ok(())
    }

    /// 窗口标题随推送热更:换成含 lyric 的模板后 wants_lyric 翻真。
    #[test]
    fn pushed_config_rebuilds_window_title() -> color_eyre::Result<()> {
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        assert!(!app.window_title.wants_lyric(), "默认模板不含 lyric");
        app.apply_pushed_config(pushed_tree(serde_json::json!({ "tui": { "window_title": {
            "template": [ { "field": "lyric" } ],
        } } }))?);
        assert!(
            app.window_title.wants_lyric(),
            "热更后模板含 lyric → 窗口标题热应用"
        );
        Ok(())
    }

    /// 固化型就地重设保留运行态:全屏形变飞行中热更拍数,逻辑态与相位都不回零。
    #[test]
    fn pushed_config_retempo_preserves_phase() -> color_eyre::Result<()> {
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        app.state.browse.fullscreen.toggle();
        for _ in 0..3 {
            app.state.browse.fullscreen.tick();
        }
        let mid = app.state.browse.fullscreen.eased_in_out();
        assert!(mid > 0, "前置:形变已起步");
        app.apply_pushed_config(pushed_tree(
            serde_json::json!({ "tui": { "animation": { "fullscreen_ms": 1000 } } }),
        )?);
        assert!(app.state.browse.fullscreen.on(), "逻辑态保留");
        assert!(
            app.state.browse.fullscreen.eased_in_out() >= mid,
            "相位不回零(retempo 只换速度)"
        );
        Ok(())
    }

    /// 防御:落型不了的推送(版本偏斜)保留现行配置,弹驻留错误卡;
    /// 下一帧好配置到来自动撤卡。
    #[test]
    fn bad_pushed_config_keeps_current_and_recovers() -> color_eyre::Result<()> {
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        app.apply_pushed_config(BusValue::Map(vec![(
            "no_such_section".to_owned(),
            BusValue::Int(1),
        )]));
        assert_eq!(
            app.keymap.lookup(KeyChord::parse("<Space>")?),
            Some(Action::TogglePlayPause),
            "失败时键表不动"
        );
        assert!(
            app.notifications.has_live_card(super::PUSH_CARD_ID),
            "失败应弹驻留错误卡"
        );
        app.apply_pushed_config(pushed_tree(serde_json::json!({}))?);
        assert!(
            !app.notifications.has_live_card(super::PUSH_CARD_ID),
            "好配置到来应撤卡"
        );
        Ok(())
    }

    /// 封面缓存预算热更:缩到 0 立即逐出已缓存原图(不清表结构、派生物联动清理)。
    #[test]
    fn pushed_config_shrinks_cover_budget_evicts() -> color_eyre::Result<()> {
        use std::sync::Arc;

        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        let url = mineral_model::MediaUrl::remote("https://x.y/c.jpg")?;
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(8, 8));
        let evicted = app.state.covers.cache.insert(&url, Arc::new(img));
        assert!(evicted.is_empty(), "预算内不逐出");
        assert_eq!(app.state.covers.cache.len(), 1, "前置:已缓存一张");
        app.apply_pushed_config(pushed_tree(
            serde_json::json!({ "tui": { "cover": { "cache": { "image": 0 } } } }),
        )?);
        assert_eq!(app.state.covers.cache.len(), 0, "预算缩到 0 应立即逐出");
        Ok(())
    }

    /// 启动期:config.lua 缺失 → init 提醒卡;文件存在 → 不弹;
    /// 启动降级告警 → 警告卡。
    #[test]
    fn startup_notifies_missing_config_and_warnings() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let missing = dir.path().join("config.lua");
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        app.notify_startup_config(Some(&missing), /*warnings*/ &[]);
        assert!(
            app.notifications.has_live_card("config.init"),
            "缺配置应弹 init 提醒卡"
        );

        std::fs::write(&missing, "return {}")?;
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        app.notify_startup_config(Some(&missing), /*warnings*/ &[]);
        assert!(
            !app.notifications.has_live_card("config.init"),
            "配置存在不该提醒 init"
        );

        // 真实坏配置产出的 warnings → 警告卡。
        std::fs::write(
            &missing,
            r#"return { tui = { behavior = { volume_step = "loud" } } }"#,
        )?;
        let (_cfg, warnings) = mineral_config::load(&missing)?;
        assert!(!warnings.is_empty(), "坏字段应产出 warning");
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        app.notify_startup_config(Some(&missing), &warnings);
        assert!(
            app.notifications.has_live_card("config.reload"),
            "启动降级告警应进警告卡"
        );
        Ok(())
    }
}
