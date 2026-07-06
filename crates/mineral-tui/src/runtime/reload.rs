//! TUI 侧配置热重载:mtime 轮询 config.lua → 重读配置 → 重建 keymap / theme / 窗口标题。
//!
//! 与 daemon 的脚本重载**互相独立**(两进程各自看文件):TUI 这条只管自己
//! 消费的配置切片;脚本 bind 键的刷新另由 daemon 推送的
//! [`Event::ScriptReloaded`](mineral_protocol::Event::ScriptReloaded) 触发
//! (见 `App::refresh_script_binds`),那才是 bind 表就绪的权威信号。
//!
//! 范围:热应用 **keys / behavior(keymap)、theme、窗口标题与 marquee 节奏**
//! (marquee 是折算成拍的快照,靠整体重建生效);其余构造期参数(动画时长 /
//! toast TTL / 布局等)不热应用,重启生效。

use std::time::{Duration, Instant, SystemTime};

use crate::runtime::marquee::Marquees;

/// mtime 轮询间隔(独立于帧率:配置文件是人手保存,1s 粒度足够)。
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// 配置重载问题卡(警告 / 失败共用)的顶替键:重复重载顶替不刷屏,
/// 干净重载主动撤卡。
const RELOAD_CARD_ID: &str = "config.reload";

/// config.lua 的 mtime 监视器(App 持有,主循环每帧喂)。
pub(crate) struct ConfigWatch {
    /// 监视目标(config.lua 路径;解析失败时为 `None`,监视禁用)。
    path: Option<std::path::PathBuf>,

    /// 上次看到的修改时间(`None` = 文件不存在)。
    last_mtime: Option<SystemTime>,

    /// 上次 stat 的时刻(限频)。
    last_poll: Instant,
}

impl ConfigWatch {
    /// 建监视器并记下当前 mtime 基线。
    pub(crate) fn new() -> Self {
        let path = mineral_paths::config_dir()
            .ok()
            .map(|d| d.join("config.lua"));
        let last_mtime = path.as_deref().and_then(mtime_of);
        Self {
            path,
            last_mtime,
            last_poll: Instant::now(),
        }
    }

    /// 每帧调用:到轮询间隔才真 stat;mtime 变了返回 `true`(调用方触发重载)。
    pub(crate) fn changed(&mut self) -> bool {
        let Some(path) = self.path.as_deref() else {
            return false;
        };
        if self.last_poll.elapsed() < POLL_INTERVAL {
            return false;
        }
        self.last_poll = Instant::now();
        let current = mtime_of(path);
        if current == self.last_mtime {
            return false;
        }
        self.last_mtime = current;
        true
    }
}

/// 读文件修改时间;文件缺失 / stat 失败为 `None`。
fn mtime_of(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl crate::app::App {
    /// 配置文件变更的重载入口(主循环在 [`ConfigWatch::changed`] 为真时调)。
    pub(crate) fn reload_config(&mut self) {
        let Ok(dir) = mineral_paths::config_dir() else {
            return;
        };
        self.reload_config_from(&dir.join("config.lua"));
    }

    /// 启动期配置提示(`run` 在 `App::new` 后调一次):
    ///   - `config_path` 不存在 → 驻留卡提醒 `mineral config init`(每次启动都提醒,
    ///     直到用户真的生成配置);
    ///   - 启动加载的降级告警 → 与热重载同一张警告卡(同 id,改好后热重载自动撤)。
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
                Some(RELOAD_CARD_ID.to_owned()),
                /*ttl*/ None,
            );
        }
    }

    /// 从指定路径重读配置并热应用 keymap / theme / 窗口标题(路径可注入,单测用)。
    ///
    /// 加载失败(IO / 全文件 eval 失败回落默认也算成功路径,由 loader 语义
    /// 决定)时保留现行配置,弹驻留错误卡(错误链逐层一行);切片级 warning
    /// 聚合成一张驻留警告卡。问题卡共用顶替键 [`RELOAD_CARD_ID`]:重复重载
    /// 顶替不刷屏,干净重载(无警告)主动撤卡——用户修好配置卡自动消失。
    ///
    /// # Params:
    ///   - `path`: config.lua 路径
    pub(crate) fn reload_config_from(&mut self, path: &std::path::Path) {
        use crate::components::toast::card::{plain_body, plain_line};
        use crate::components::toast::notifications::{TextTint, tinted_text_item};
        let (cfg, warnings) = match mineral_config::load(path) {
            Ok(loaded) => loaded,
            Err(e) => {
                mineral_log::warn!(
                    target: "tui",
                    error = mineral_log::chain(&e),
                    "配置重载失败,保留现行配置"
                );
                let mut lines = e.chain().map(ToString::to_string).collect::<Vec<String>>();
                lines.push("keeping current config".to_owned());
                self.notifications.push_card(
                    TextTint::Error,
                    plain_line("config reload failed"),
                    plain_body(lines),
                    Some(RELOAD_CARD_ID.to_owned()),
                    /*ttl*/ None,
                );
                return;
            }
        };
        if warnings.is_empty() {
            self.notifications.dismiss_card_by_id(RELOAD_CARD_ID);
        } else {
            for warning in &warnings {
                mineral_log::warn!(target: "tui", warning = %warning, "配置重载 warning");
            }
            let lines = warnings.iter().map(ToString::to_string);
            self.notifications.push_card(
                TextTint::Warn,
                plain_line("config.lua warnings"),
                plain_body(lines),
                Some(RELOAD_CARD_ID.to_owned()),
                /*ttl*/ None,
            );
        }
        let cfg = std::sync::Arc::new(cfg);
        self.theme =
            std::sync::Arc::new(crate::render::theme::Theme::from_config(cfg.tui().theme()));
        self.state.cfg = cfg;
        self.window_title =
            crate::runtime::window_title::WindowTitle::new(self.state.cfg.tui().window_title());
        let anim = self.state.cfg.tui().animation();
        self.state.marquees = Marquees::from_config(anim.marquee(), *anim.frame_tick_ms());
        self.rebuild_keymap();
        mineral_log::info!(target: "tui", "配置已重载(keymap / theme / 窗口标题 / marquee)");
        self.notifications
            .flash(tinted_text_item("配置已重载".to_owned(), TextTint::Normal));
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

    use crate::runtime::action::Action;
    use crate::test_support::app_with_queue;

    /// 重载后 keymap / theme 热应用:改键绑定 + 主题色,reload 后即生效;
    /// 通知层收到「配置已重载」提示。
    #[test]
    fn reload_applies_keymap_and_theme() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.lua");
        std::fs::write(
            &path,
            r##"return { tui = {
                keys = { play_pause = "w" },
                theme = { accent = "#ff0000" },
            } }"##,
        )?;
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        let old_accent = app.theme.accent;
        assert_eq!(
            app.keymap.lookup(KeyChord::parse("<Space>")?),
            Some(Action::TogglePlayPause),
            "重载前默认绑定"
        );
        app.reload_config_from(&path);
        assert_eq!(
            app.keymap.lookup(KeyChord::parse("w")?),
            Some(Action::TogglePlayPause),
            "重载后新键生效"
        );
        assert_eq!(
            app.keymap.lookup(KeyChord::parse("<Space>")?),
            None,
            "旧键整体替换"
        );
        assert_ne!(app.theme.accent, old_accent, "主题热应用");
        assert!(app.notifications.entry_count() >= 1, "应有重载提示");
        Ok(())
    }

    /// marquee 节奏随配置热重载:mode 改 off,reload 后溢出标题恒零相位
    /// (证明 Marquees 用新配置重建了,而非沿用启动折算的快照)。
    #[test]
    fn reload_rebuilds_marquee_tempo() -> color_eyre::Result<()> {
        use crate::runtime::marquee::Slot;

        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.lua");
        std::fs::write(
            &path,
            r#"return { tui = { animation = { marquee = { mode = "off" } } } }"#,
        )?;
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
        assert!(scrolled > 0, "重载前默认 loop 应在滚动");
        app.reload_config_from(&path);
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
        assert_eq!(after, 0, "重载为 off 后应恒零相位");
        Ok(())
    }

    /// 窗口标题随配置热重载:换成含 lyric 的模板,reload 后 wants_lyric 翻真
    /// (证明 window_title 用新配置重建了,而非沿用旧的)。
    #[test]
    fn reload_rebuilds_window_title() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.lua");
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        assert!(!app.window_title.wants_lyric(), "默认模板不含 lyric");
        std::fs::write(
            &path,
            r#"return { tui = { window_title = { template = { { field = "lyric" } } } } }"#,
        )?;
        app.reload_config_from(&path);
        assert!(
            app.window_title.wants_lyric(),
            "重载后模板含 lyric → 窗口标题热应用"
        );
        Ok(())
    }

    /// 加载失败(IO 错误:路径是目录)保留现行配置,弹驻留错误卡。
    #[test]
    fn reload_failure_keeps_current_config() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        app.reload_config_from(dir.path()); // 目录不是文件 → load Err
        assert_eq!(
            app.keymap.lookup(KeyChord::parse("<Space>")?),
            Some(Action::TogglePlayPause),
            "失败时键表不动"
        );
        assert!(
            app.notifications.has_live_card(super::RELOAD_CARD_ID),
            "失败应弹驻留错误卡"
        );
        Ok(())
    }

    /// 启动期:config.lua 缺失 → init 提醒卡;文件存在 → 不弹;
    /// 启动降级告警 → 与热重载同 id 的警告卡。
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

        // 真实坏配置产出的 warnings → 与热重载同一张警告卡。
        std::fs::write(
            &missing,
            r#"return { tui = { behavior = { volume_step = "loud" } } }"#,
        )?;
        let (_cfg, warnings) = mineral_config::load(&missing)?;
        assert!(!warnings.is_empty(), "坏字段应产出 warning");
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        app.notify_startup_config(Some(&missing), &warnings);
        assert!(
            app.notifications.has_live_card(super::RELOAD_CARD_ID),
            "启动降级告警应进警告卡"
        );
        Ok(())
    }

    /// 坏字段配置:警告升级为驻留卡(同 id 顶替不堆叠);修好后干净重载主动撤卡。
    #[test]
    fn reload_warning_card_replaces_then_clears_on_clean_reload() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("config.lua");
        std::fs::write(
            &path,
            r#"return { tui = { behavior = { volume_step = "loud" } } }"#,
        )?;
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        app.reload_config_from(&path);
        assert!(
            app.notifications.has_live_card(super::RELOAD_CARD_ID),
            "坏字段应弹警告卡"
        );
        app.reload_config_from(&path);
        assert_eq!(app.notifications.card_count(), 1, "重复重载同 id 顶替");

        std::fs::write(&path, "return {}")?;
        app.reload_config_from(&path);
        assert!(
            !app.notifications.has_live_card(super::RELOAD_CARD_ID),
            "干净重载应撤卡"
        );
        Ok(())
    }
}
