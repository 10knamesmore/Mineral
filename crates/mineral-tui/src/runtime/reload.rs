//! TUI 侧配置热重载:mtime 轮询 config.lua → 重读配置 → 重建 keymap / theme。
//!
//! 与 daemon 的脚本重载**互相独立**(两进程各自看文件):TUI 这条只管自己
//! 消费的配置切片;脚本 bind 键的刷新另由 daemon 推送的
//! [`Event::ScriptReloaded`](mineral_protocol::Event::ScriptReloaded) 触发
//! (见 `App::refresh_script_binds`),那才是 bind 表就绪的权威信号。
//!
//! 范围:本期热应用 **keys / behavior(keymap)与 theme**;构造期参数
//! (动画时长 / toast TTL / 布局等)不热应用,重启生效。

use std::time::{Duration, Instant, SystemTime};

/// mtime 轮询间隔(独立于帧率:配置文件是人手保存,1s 粒度足够)。
const POLL_INTERVAL: Duration = Duration::from_secs(1);

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

    /// 从指定路径重读配置并热应用 keymap / theme(路径可注入,单测用)。
    ///
    /// 加载失败(IO / 全文件 eval 失败回落默认也算成功路径,由 loader 语义
    /// 决定)时保留现行配置并 toast 提示;切片级 warning 逐条提示。
    ///
    /// # Params:
    ///   - `path`: config.lua 路径
    pub(crate) fn reload_config_from(&mut self, path: &std::path::Path) {
        use crate::components::toast::notifications::{TextTint, tinted_text_item};
        let (cfg, warnings) = match mineral_config::load(path) {
            Ok(loaded) => loaded,
            Err(e) => {
                mineral_log::warn!(
                    target: "tui",
                    error = mineral_log::chain(&e),
                    "配置重载失败,保留现行配置"
                );
                self.notifications.flash(tinted_text_item(
                    "配置重载失败,保留现行配置(详见日志)".to_owned(),
                    TextTint::Error,
                ));
                return;
            }
        };
        for warning in &warnings {
            mineral_log::warn!(target: "tui", warning = %warning, "配置重载 warning");
            self.notifications
                .flash(tinted_text_item(warning.to_string(), TextTint::Warn));
        }
        let cfg = std::sync::Arc::new(cfg);
        self.theme =
            std::sync::Arc::new(crate::render::theme::Theme::from_config(cfg.tui().theme()));
        self.state.cfg = cfg;
        self.rebuild_keymap();
        mineral_log::info!(target: "tui", "配置已重载(keymap / theme)");
        self.notifications
            .flash(tinted_text_item("配置已重载".to_owned(), TextTint::Normal));
    }

    /// daemon 推送 `ScriptReloaded` 后刷新脚本 bind 键(配置部分不动,
    /// 用现行 `state.cfg` 重建 keymap 再合新 bind 表)。
    pub(crate) fn refresh_script_binds(&mut self) {
        self.rebuild_keymap();
    }

    /// 以现行配置重建 keymap 并合入 daemon 的 bind 表。
    fn rebuild_keymap(&mut self) {
        let tui_cfg = self.state.cfg.tui();
        let mut keymap =
            crate::runtime::keymap::Keymap::from_config(tui_cfg.keys(), tui_cfg.behavior());
        keymap.append_script_binds(&self.client.script_binds());
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
                keys = { play_pause = "x" },
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
            app.keymap.lookup(KeyChord::parse("x")?),
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

    /// 加载失败(IO 错误:路径是目录)保留现行配置,toast 报错。
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
        Ok(())
    }
}
