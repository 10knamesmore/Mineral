//! daemon.lua 热重载：轮询单个文件，成功启动新运行时后替换旧脚本和私有配置。
//!
//! 求值、setup 或线程创建失败保留旧运行时与配置；删除文件清除 hook 并恢复 daemon 默认值。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mineral_protocol::{Event, FailureNotice, TextSpan, ToastKind};
use mineral_script::{ScriptHost, ScriptRuntime, ScriptSender, install_daemon_api};
use tokio::sync::mpsc::UnboundedSender;

use crate::script_bridge::ScriptReloadParts;

/// daemon.lua 修改时间轮询间隔。
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// 干净重载后撤销配置问题通知的标识。
const CONFIG_NOTICE_ID: &str = "config.reload";

/// 轮询指定 daemon.lua；不读取 tui.lua 或其他配置文件。
///
/// # Params:
///   - `daemon_path`: 用户 daemon.lua 路径
///   - `runtime`: 当前运行时，持有到成功替换或文件删除
///   - `sender`: 跨重载复用的投递入口
///   - `parts`: 命令、诊断、看门狗与私有配置落点
pub fn spawn_script_reloader(
    daemon_path: PathBuf,
    runtime: Option<ScriptRuntime>,
    sender: ScriptSender,
    parts: ScriptReloadParts,
) {
    tokio::spawn(async move {
        let initialized = tokio::task::spawn_blocking(move || Reloader {
            last: mtime_of(&daemon_path),
            path: daemon_path,
            runtime,
            sender,
            parts,
        })
        .await;
        let mut reloader = match initialized {
            Ok(reloader) => reloader,
            Err(error) => {
                mineral_log::error!(target: "script", error = mineral_log::chain(&error), "daemon configuration watcher could not initialize");
                return;
            }
        };
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            let polled = tokio::task::spawn_blocking(move || {
                reloader.poll();
                reloader
            })
            .await;
            reloader = match polled {
                Ok(reloader) => reloader,
                Err(error) => {
                    mineral_log::error!(target: "script", error = mineral_log::chain(&error), "daemon configuration watcher stopped");
                    return;
                }
            };
        }
    });
}

/// 在阻塞任务之间移交同一个配置监听器;异步任务只负责等待轮询节拍。
struct Reloader {
    /// 唯一监听的 daemon.lua 文件。
    path: PathBuf,

    /// 当前已激活的脚本线程,失败重载不替换它。
    runtime: Option<ScriptRuntime>,

    /// 跨重载保持的命令投递入口。
    sender: ScriptSender,

    /// 配置提交、统计与通知所需的服务落点。
    parts: ScriptReloadParts,

    /// 已观察的文件版本;None 表示缺失或 stat 失败。
    last: Option<SystemTime>,
}

impl Reloader {
    /// 同步观察一个文件版本并完成整次重载;不占用异步 worker。
    fn poll(&mut self) {
        let current = mtime_of(&self.path);
        if current == self.last {
            return;
        }
        self.last = current;
        mineral_log::info!(target: "script", daemon_path = %self.path.display(), "daemon.lua 变更,开始重载");
        reload_once(&self.path, &mut self.runtime, &self.sender, &self.parts);
    }
}

/// 文件缺失或 stat 失败为 None；创建、删除与重建均参与重载判断。
fn mtime_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// 加载并启动新 runtime；setup 成功前不丢弃旧 runtime 或更新有效配置。
fn reload_once(
    daemon_path: &Path,
    runtime: &mut Option<ScriptRuntime>,
    sender: &ScriptSender,
    parts: &ScriptReloadParts,
) {
    let host = ScriptHost::new(parts.cmd_tx.clone(), parts.push_tx.clone());
    let loaded =
        match crate::config::load_daemon_with_vm(daemon_path, |lua| install_daemon_api(lua, &host))
        {
            Ok(loaded) => loaded,
            Err(error) => {
                report_reload_failure(parts, runtime.is_some(), mineral_log::chain(&error));
                return;
            }
        };
    let Some(lua) = loaded.vm else {
        if loaded.warnings.is_empty() {
            sender.detach();
            *runtime = None;
            (parts.apply_config_base)(loaded.tree);
            let _ = parts.push_tx.send(Event::DismissToast {
                id: CONFIG_NOTICE_ID.to_owned(),
            });
            mineral_log::info!(target: "script", "daemon.lua 已删除,清除脚本并恢复 daemon 默认配置");
            record_script_lifecycle(parts, mineral_stats::ScriptEvent::ReloadOk, None);
            return;
        }
        report_config_warnings(&parts.push_tx, &loaded.warnings);
        let detail = loaded
            .warnings
            .iter()
            .map(|warning| mineral_log::chain(warning))
            .collect::<Vec<_>>()
            .join("; ");
        report_reload_failure(parts, runtime.is_some(), detail);
        return;
    };
    crate::script_bridge::seed_web_urls(&lua, &parts.web_urls);
    match ScriptRuntime::spawn(lua, host, parts.watchdog, sender) {
        Ok(new_runtime) => {
            // spawn 已原子挂接新 sender；Drop 旧 runtime 只停止它自己的线程。
            *runtime = Some(new_runtime);
            (parts.apply_config_base)(loaded.tree);
            mineral_log::info!(target: "script", "daemon 配置与音乐脚本已热重载");
            let _ = parts.push_tx.send(Event::DismissToast {
                id: CONFIG_NOTICE_ID.to_owned(),
            });
            toast(&parts.push_tx, ToastKind::Info, "脚本已热重载".to_owned());
            record_script_lifecycle(parts, mineral_stats::ScriptEvent::ReloadOk, None);
        }
        Err(error) => report_reload_failure(parts, runtime.is_some(), mineral_log::chain(&error)),
    }
}

/// 保留旧运行时并发送结构化失败类别，诊断只写 daemon 日志和 analytics。
fn report_reload_failure(parts: &ScriptReloadParts, previous_kept: bool, detail: String) {
    mineral_log::warn!(target: "script", error = detail, previous_kept, "daemon 重载失败,保持旧运行状态");
    let _ = parts
        .push_tx
        .send(Event::Failure(FailureNotice::ScriptReloadFailed {
            previous_kept,
        }));
    record_script_lifecycle(parts, mineral_stats::ScriptEvent::ReloadFail, Some(detail));
}

/// 记录重载成功或失败；历史 analytics 保留诊断。
fn record_script_lifecycle(
    parts: &ScriptReloadParts,
    event: mineral_stats::ScriptEvent,
    detail: Option<String>,
) {
    parts.stats.event(mineral_stats::StatsEvent::System(
        mineral_stats::SystemEvent::ScriptLifecycle { event, detail },
    ));
}

/// 经 event hub 发送重载通知。
fn toast(push_tx: &UnboundedSender<Event>, kind: ToastKind, content: String) {
    let _ = push_tx.send(Event::Toast {
        kind,
        content: vec![TextSpan::plain(content)],
        id: Some("script.reload".to_owned()),
        ttl_secs: None,
    });
}

/// 推送配置告警路径，完整诊断留在日志。
fn report_config_warnings(
    push_tx: &UnboundedSender<Event>,
    warnings: &[mineral_config::ConfigWarning],
) {
    for warning in warnings {
        mineral_log::warn!(target: "script", error = mineral_log::chain(warning), "daemon 配置校验失败");
    }
    let fields = warnings
        .iter()
        .filter_map(|warning| match warning {
            mineral_config::ConfigWarning::Deserialize { path, .. } => path.clone(),
            _ => None,
        })
        .collect();
    let _ = push_tx.send(Event::Failure(FailureNotice::ConfigRejected { fields }));
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mineral_script::{BeforeStreamCtx, HookDecision, HookMode, WatchdogConfig};
    use mineral_test::song;
    use parking_lot::Mutex;
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    use super::*;

    /// 测试回调仍受 watchdog 约束，但留出线程调度余量。
    fn lax_watchdog() -> WatchdogConfig {
        WatchdogConfig::builder()
            .instruction_interval(10_000)
            .soft_wall(std::time::Duration::from_millis(200))
            .hard_wall(std::time::Duration::from_secs(1))
            .build()
    }

    /// 在一份 daemon 文件中配置具名变换和音乐 hook。
    fn transform_config(occurrence: usize) -> String {
        format!(
            r#"return {{
            audio = {{ volume = 55 }},
            queue = {{ transforms = {{{{
                name = "pick",
                transform = function(songs) return {{songs[{occurrence}]}} end,
            }}}} }},
            setup = function(api)
                api.hook("before_stream", function() return false end)
            end
        }}"#
        )
    }

    /// 临时 daemon.lua 与真实运行时；只替换配置应用的外部落点。
    struct Rig {
        /// 临时文件生命周期。
        _dir: tempfile::TempDir,

        /// 唯一 daemon 文件路径。
        path: PathBuf,

        /// 当前运行时。
        runtime: Option<ScriptRuntime>,

        /// 跨重载投递句柄。
        sender: ScriptSender,

        /// 重载装配件。
        parts: ScriptReloadParts,

        /// 最近应用成功的私有配置底树。
        base: Arc<Mutex<serde_json::Value>>,

        /// 脚本命令接收端，验证失败 setup 没有副作用。
        cmd_rx: UnboundedReceiver<mineral_script::ScriptCmd>,

        /// 重载诊断接收端。
        push_rx: UnboundedReceiver<Event>,
    }

    impl Rig {
        /// 与 daemon 启动相同的加载和激活路径。
        fn boot(source: &str) -> color_eyre::Result<Self> {
            Self::boot_with_stats(source, crate::StatsRecorder::disabled())
        }

        /// 注入 analytics recorder。
        fn boot_with_stats(source: &str, stats: crate::StatsRecorder) -> color_eyre::Result<Self> {
            let dir = tempfile::tempdir()?;
            let path = dir.path().join("daemon.lua");
            std::fs::write(&path, source)?;
            let (cmd_tx, cmd_rx) = unbounded_channel();
            let (push_tx, push_rx) = unbounded_channel();
            let host = ScriptHost::new(cmd_tx.clone(), push_tx.clone());
            let loaded =
                crate::config::load_daemon_with_vm(&path, |lua| install_daemon_api(lua, &host))?;
            assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
            let base = Arc::new(Mutex::new(loaded.tree));
            let config_base = Arc::clone(&base);
            let sender = ScriptSender::detached();
            let lua = loaded
                .vm
                .ok_or_else(|| color_eyre::eyre::eyre!("initial daemon VM unavailable"))?;
            let runtime = Some(ScriptRuntime::spawn(lua, host, lax_watchdog(), &sender)?);
            Ok(Self {
                _dir: dir,
                path,
                runtime,
                sender,
                parts: ScriptReloadParts {
                    cmd_tx,
                    push_tx,
                    watchdog: lax_watchdog(),
                    web_urls: Vec::new(),
                    apply_config_base: Arc::new(move |tree| *config_base.lock() = tree),
                    stats,
                },
                base,
                cmd_rx,
                push_rx,
            })
        }

        /// 同步文件改写 helper，不在 async 测试体内执行阻塞文件调用。
        fn rewrite_and_reload(&mut self, source: &str) -> color_eyre::Result<()> {
            std::fs::write(&self.path, source)?;
            reload_once(&self.path, &mut self.runtime, &self.sender, &self.parts);
            Ok(())
        }

        /// 对同一歌曲快照运行 before_stream。
        async fn intercept(&self) -> HookDecision {
            self.sender
                .intercept_stream(
                    BeforeStreamCtx::playable(
                        song("a"),
                        mineral_model::BitRate::Standard,
                        HookMode::Immediate,
                        None,
                    ),
                    std::time::Duration::from_secs(1),
                )
                .await
        }

        /// 按稳定名称运行队列变换。
        async fn transform(&self) -> color_eyre::Result<Vec<mineral_model::SongId>> {
            Ok(self
                .sender
                .queue_transform("pick".to_owned(), vec![song("a"), song("b")], 0, None)
                .await??)
        }
    }

    #[tokio::test]
    async fn daemon_reload_replaces_music_hooks() -> color_eyre::Result<()> {
        let mut rig = Rig::boot(
            r#"return {setup = function(api)
            api.hook("before_stream", function() return false end)
        end}"#,
        )?;
        assert!(matches!(rig.intercept().await, HookDecision::Skip { .. }));
        rig.rewrite_and_reload(
            r#"return {setup = function(api)
            api.hook("before_stream", function() return nil end)
        end}"#,
        )?;
        assert_eq!(rig.intercept().await, HookDecision::Continue);
        Ok(())
    }

    #[tokio::test]
    async fn reload_replaces_named_transform_callbacks_and_base() -> color_eyre::Result<()> {
        let mut rig = Rig::boot(&transform_config(1))?;
        assert_eq!(rig.transform().await?, vec![song("a").id]);
        rig.rewrite_and_reload(&transform_config(2))?;
        assert_eq!(rig.transform().await?, vec![song("b").id]);
        assert_eq!(
            rig.base.lock().pointer("/audio/volume"),
            Some(&serde_json::json!(55))
        );
        rig.rewrite_and_reload("return {audio = {volume = 42}}")?;
        assert!(rig.transform().await.is_err());
        assert_eq!(
            rig.base.lock().pointer("/audio/volume"),
            Some(&serde_json::json!(42))
        );
        Ok(())
    }

    #[tokio::test]
    async fn invalid_file_and_failed_setup_keep_old_hooks_transforms_and_config()
    -> color_eyre::Result<()> {
        let mut rig = Rig::boot(&transform_config(1))?;
        let base = rig.base.lock().clone();
        for source in [
            "this is not lua ((",
            "return { daemon = { setup = function(api) api.player.stop() end } }",
            "return { tui = {} }",
            "return { setup = false }",
            r#"return {audio = {volume = 7}, setup = function(api)
                api.player.stop()
                api.config.override("audio.volume", 7)
                error("setup failed")
            end}"#,
        ] {
            rig.rewrite_and_reload(source)?;
            assert!(matches!(rig.intercept().await, HookDecision::Skip { .. }));
            assert_eq!(rig.transform().await?, vec![song("a").id]);
            assert_eq!(*rig.base.lock(), base);
            assert!(rig.cmd_rx.try_recv().is_err());
            let mut failure = false;
            while let Ok(event) = rig.push_rx.try_recv() {
                failure |= matches!(
                    event,
                    Event::Failure(FailureNotice::ScriptReloadFailed {
                        previous_kept: true
                    })
                );
            }
            assert!(failure);
        }
        Ok(())
    }

    #[tokio::test]
    async fn old_query_result_cannot_resolve_a_new_runtime_callback() -> color_eyre::Result<()> {
        let source = r#"return {setup = function(api)
            api.store.get("netease:1", "value", function(value)
                api.player.set_volume(value)
            end)
        end}"#;
        let mut rig = Rig::boot(source)?;
        let mineral_script::ScriptCmd::StoreGet { query: old, .. } = rig.cmd_rx.try_recv()? else {
            color_eyre::eyre::bail!("expected initial store query");
        };
        rig.rewrite_and_reload(source)?;
        let mineral_script::ScriptCmd::StoreGet { query: current, .. } = rig.cmd_rx.try_recv()?
        else {
            color_eyre::eyre::bail!("expected reloaded store query");
        };
        rig.sender.resolve(
            old,
            mineral_script::ResolveValue::Store(mineral_model::StoreValue::Int(11)),
        );
        rig.sender.resolve(
            current,
            mineral_script::ResolveValue::Store(mineral_model::StoreValue::Int(22)),
        );
        assert_eq!(rig.intercept().await, HookDecision::Continue);
        assert!(matches!(
            rig.cmd_rx.try_recv(),
            Ok(mineral_script::ScriptCmd::SetVolume(22))
        ));
        assert!(rig.cmd_rx.try_recv().is_err());
        Ok(())
    }

    #[tokio::test]
    async fn deleting_daemon_file_removes_hooks_and_restores_defaults() -> color_eyre::Result<()> {
        let mut rig = Rig::boot(&transform_config(1))?;
        tokio::fs::remove_file(&rig.path).await?;
        reload_once(&rig.path, &mut rig.runtime, &rig.sender, &rig.parts);
        assert!(!rig.sender.is_attached());
        assert!(rig.runtime.is_none());
        assert_eq!(*rig.base.lock(), crate::config::default_daemon_tree()?);
        assert_eq!(rig.intercept().await, HookDecision::Continue);
        assert!(rig.transform().await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn reload_upgrades_from_no_script() -> color_eyre::Result<()> {
        let mut rig = Rig::boot("return {}")?;
        rig.runtime = None;
        rig.sender.detach();
        rig.rewrite_and_reload(&transform_config(1))?;
        assert!(rig.sender.is_attached());
        assert!(matches!(rig.intercept().await, HookDecision::Skip { .. }));
        assert_eq!(rig.transform().await?, vec![song("a").id]);
        Ok(())
    }

    #[test]
    fn reload_watches_only_the_daemon_file() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("daemon.lua");
        assert_eq!(mtime_of(&path), None);
        std::fs::write(dir.path().join("tui.lua"), "invalid tui lua ((")?;
        assert_eq!(mtime_of(&path), None);
        std::fs::write(&path, "return {}")?;
        let created = mtime_of(&path);
        assert!(created.is_some());
        std::fs::remove_file(&path)?;
        assert_ne!(mtime_of(&path), created);
        Ok(())
    }

    /// 成功与失败继续保留 script_lifecycle analytics。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reload_records_script_lifecycle() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = mineral_stats::StatsStore::open(&dir.path().join("stats.db")).await?;
        let params = crate::params_from_config(crate::config::DaemonConfig::defaults()?.stats());
        let (recorder, _actor) = crate::StatsRecorder::spawn(store.clone(), params);
        let mut rig = Rig::boot_with_stats("return {}", recorder)?;
        rig.rewrite_and_reload("return {}")?;
        rig.rewrite_and_reload("this is not lua ((")?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while store.status().await?.events < 2 {
            if std::time::Instant::now() > deadline {
                color_eyre::eyre::bail!("script lifecycle records did not arrive");
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        Ok(())
    }
}
