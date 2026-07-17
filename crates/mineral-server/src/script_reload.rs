//! config.lua 热重载:mtime 轮询 → 重 eval 到新 VM → 原子换脚本线程。
//!
//! 换 VM 语义:eval **成功**才停老线程、起新线程(新注册表整体替换,
//! 无中间态双注册);eval 失败保留旧 VM 照常跑(不空窗),错误经 toast
//! 提示。`ScriptSender` 是间接句柄,daemon 各处持有者无感。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mineral_protocol::{Event, TextSpan, ToastKind};
use mineral_script::{ScriptHost, ScriptRuntime, ScriptSender, install_api};
use tokio::sync::mpsc::UnboundedSender;

use crate::script_bridge::ScriptReloadParts;

/// mtime 轮询间隔。
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// 配置问题驻留卡的顶替键:重复重载顶替不刷屏,干净重载主动撤卡
/// (client 侧同 id 的存活卡被 [`Event::DismissToast`] 撤掉)。
const CONFIG_CARD_ID: &str = "config.reload";

/// 起热重载任务:轮询 `config_path` 的修改时间,变更即重载脚本 VM。
///
/// 持有 `runtime`(当前脚本线程)的所有权直到 daemon 退出;重载成功时
/// 旧线程在此停机(Stop + join)、新线程顶上。
///
/// # Params:
///   - `config_path`: 用户 config.lua 路径
///   - `runtime`: 当前脚本线程句柄(无脚本为 `None`,重载可升级为有)
///   - `sender`: daemon 侧投递句柄(重载换内层,持有者无感)
///   - `parts`: 泵接线时拆出的通道发送端 + 看门狗参数
pub fn spawn_script_reloader(
    config_path: PathBuf,
    mut runtime: Option<ScriptRuntime>,
    sender: ScriptSender,
    parts: ScriptReloadParts,
) {
    tokio::spawn(async move {
        let mut last = mtime_of(&config_path);
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            let current = mtime_of(&config_path);
            if current == last {
                continue;
            }
            last = current;
            mineral_log::info!(target: "script", path = %config_path.display(), "config.lua 变更,重载脚本");
            reload_once(&config_path, &mut runtime, &sender, &parts);
        }
    });
}

/// 读文件修改时间;文件缺失 / stat 失败为 `None`(与「存在」可区分,
/// 文件删除→重建也会触发重载)。
fn mtime_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// 执行一次重载:新 host + 新 VM eval → 成功则换线程,失败保留旧。
///
/// # Params:
///   - `config_path`: 用户 config.lua 路径
///   - `runtime`: 当前脚本线程槽(成功时旧的在此 drop = Stop + join)
///   - `sender`: daemon 侧投递句柄
///   - `parts`: 通道发送端 + 看门狗参数
fn reload_once(
    config_path: &Path,
    runtime: &mut Option<ScriptRuntime>,
    sender: &ScriptSender,
    parts: &ScriptReloadParts,
) {
    // 新 host 复用同两条通道:泵与 daemon 侧持有的发送端全部不动。
    let host = ScriptHost::new(parts.cmd_tx.clone(), parts.push_tx.clone());
    // eval 前播种当前属性值:observe「订阅即回放」与顶层 get 立即可用。
    // 已知小竞窗:eval 窗口内的属性变更投给垂死的旧 VM,新 VM 种子略旧 ——
    // 实际只波及 position(秒级自愈),不值得二次 diff。
    host.seed_props((parts.props_snapshot)());
    let loaded = mineral_config::load_with_vm(config_path, |lua| {
        install_api(lua, &host).map_err(color_eyre::Report::new)
    });
    let loaded = match loaded {
        Ok(parts) => parts,
        Err(e) => {
            // 配置目录级故障(读目录失败等):保留旧 VM,报错。
            mineral_log::warn!(
                target: "script",
                error = mineral_log::chain(&e),
                "脚本重载失败,保留旧脚本"
            );
            toast(
                &parts.push_tx,
                ToastKind::Error,
                format!("脚本重载失败,保留旧脚本:{e}"),
            );
            record_script_lifecycle(
                parts,
                mineral_stats::ScriptEvent::ReloadFail,
                Some(mineral_log::chain(&e)),
            );
            return;
        }
    };
    let Some(lua) = loaded.vm else {
        // eval 失败 / 文件缺失:保留旧 VM 照常跑,报错给用户。配置侧同语义:
        // **不**下发回落默认的树,有效配置保留上一份好的;告警进驻留卡
        // (同 id 顶替不刷屏,修好后干净重载撤卡)。
        let detail = loaded.warnings.first().map_or_else(
            || "config.lua 缺失或未通过求值".to_owned(),
            std::string::ToString::to_string,
        );
        mineral_log::warn!(target: "script", detail, "脚本重载失败,保留旧脚本");
        warning_card(&parts.push_tx, &loaded.warnings);
        toast(
            &parts.push_tx,
            ToastKind::Error,
            format!("脚本重载失败,保留旧脚本:{detail}"),
        );
        record_script_lifecycle(parts, mineral_stats::ScriptEvent::ReloadFail, Some(detail));
        return;
    };
    // 新 VM 重新 seed 各源网页链接模板(caps 启动后不变,直接复用装配时那份)。
    crate::script_bridge::seed_web_urls(&lua, &parts.web_urls);
    // 先停老线程再起新:换 VM 窗口内 sender 短暂指向已停线程,投递静默丢
    // (脚本是旁路增强,丢这一拍无害);失败路径不走到这里,无空窗。
    *runtime = None;
    match ScriptRuntime::spawn(lua, host, parts.watchdog, sender) {
        Ok(new_runtime) => {
            *runtime = Some(new_runtime);
            mineral_log::info!(target: "script", "脚本已热重载");
            // 新合成树交配置宿主:重算有效树 + 推送订阅 client(变了才推)。
            // 干净重载主动撤问题卡(vm 就绪必然无 warnings,加载失败同沉语义)。
            (parts.apply_config_base)(loaded.tree);
            let _ = parts.push_tx.send(Event::DismissToast {
                id: CONFIG_CARD_ID.to_owned(),
            });
            // 先发就绪信号再发 toast:client 收到 ScriptReloaded 重拉 binds 时表已就绪。
            let _ = parts.push_tx.send(Event::ScriptReloaded);
            toast(&parts.push_tx, ToastKind::Info, "脚本已热重载".to_owned());
            record_script_lifecycle(parts, mineral_stats::ScriptEvent::ReloadOk, None);
        }
        Err(e) => {
            // OS 线程起不来(极端):脚本从有到无,挂接摘掉让触发面报"未启用"。
            sender.detach();
            mineral_log::error!(
                target: "script",
                error = mineral_log::chain(&e),
                "重载后脚本线程启动失败,脚本不可用"
            );
            toast(
                &parts.push_tx,
                ToastKind::Error,
                "脚本线程启动失败,脚本不可用(详见日志)".to_owned(),
            );
            record_script_lifecycle(
                parts,
                mineral_stats::ScriptEvent::ReloadFail,
                Some(mineral_log::chain(&e)),
            );
        }
    }
}

/// 记一次脚本热重载生命周期(script_lifecycle;系统域,无 actor)。
fn record_script_lifecycle(
    parts: &ScriptReloadParts,
    event: mineral_stats::ScriptEvent,
    detail: Option<String>,
) {
    parts.stats.event(mineral_stats::StatsEvent::System(
        mineral_stats::SystemEvent::ScriptLifecycle { event, detail },
    ));
}

/// 经脚本推送通道发一条 toast(泵汇入 event hub 下发 client)。
fn toast(push_tx: &UnboundedSender<Event>, kind: ToastKind, content: String) {
    let _ = push_tx.send(Event::Toast {
        kind,
        content: vec![TextSpan::plain(content)],
        id: Some("script.reload".to_owned()),
        ttl_secs: None,
    });
}

/// 配置告警驻留卡(同 [`CONFIG_CARD_ID`] 顶替):逐条 warning 一行。
fn warning_card(push_tx: &UnboundedSender<Event>, warnings: &[mineral_config::ConfigWarning]) {
    if warnings.is_empty() {
        return;
    }
    let _ = push_tx.send(Event::Card {
        kind: ToastKind::Warn,
        id: Some(CONFIG_CARD_ID.to_owned()),
        title: vec![TextSpan::plain("config.lua warnings")],
        body: warnings
            .iter()
            .map(|w| vec![TextSpan::plain(w.to_string())])
            .collect(),
        ttl_secs: None,
    });
}

#[cfg(test)]
mod tests {
    use mineral_script::WatchdogConfig;
    use tokio::sync::mpsc::unbounded_channel;

    use super::*;

    /// 宽松看门狗。
    fn lax_watchdog() -> WatchdogConfig {
        WatchdogConfig::builder()
            .instruction_interval(10_000)
            .soft_wall(std::time::Duration::from_millis(200))
            .hard_wall(std::time::Duration::from_secs(1))
            .build()
    }

    /// 起一套隔离的重载件:临时 config.lua + 通道 + detached sender。
    struct Rig {
        /// 临时目录(守住生命周期)。
        _dir: tempfile::TempDir,

        /// config.lua 路径。
        path: PathBuf,

        /// 当前脚本线程槽。
        runtime: Option<ScriptRuntime>,

        /// daemon 侧投递句柄。
        sender: ScriptSender,

        /// 重载件。
        parts: ScriptReloadParts,
    }

    impl Rig {
        /// 写初始 config.lua 并完成首次装配(走与 daemon 入口同构的流程)。
        fn boot(initial_lua: &str) -> color_eyre::Result<Self> {
            Self::boot_with_stats(initial_lua, crate::StatsRecorder::disabled())
        }

        /// 同 [`Self::boot`],但注入指定埋点句柄(script_lifecycle 断言用)。
        fn boot_with_stats(
            initial_lua: &str,
            stats: crate::StatsRecorder,
        ) -> color_eyre::Result<Self> {
            let dir = tempfile::tempdir()?;
            let path = dir.path().join("config.lua");
            std::fs::write(&path, initial_lua)?;
            let (cmd_tx, _cmd_rx) = unbounded_channel();
            let (push_tx, _push_rx) = unbounded_channel();
            let host = ScriptHost::new(cmd_tx.clone(), push_tx.clone());
            let loaded = mineral_config::load_with_vm(&path, |lua| {
                install_api(lua, &host).map_err(color_eyre::Report::new)
            })?;
            assert!(
                loaded.warnings.is_empty(),
                "初始配置不应有 warning: {:?}",
                loaded.warnings
            );
            let sender = ScriptSender::detached();
            let lua = loaded
                .vm
                .ok_or_else(|| color_eyre::eyre::eyre!("初始 VM 应就绪"))?;
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
                    // 固定快照源:替代真 PlayerCore 的 PropsWatch(播种语义见
                    // reload_seeds_props_into_new_vm)。
                    props_snapshot: std::sync::Arc::new(|| {
                        vec![(
                            mineral_script::PropKey::PlayerVolume,
                            mineral_script::PropValue::Int(42),
                        )]
                    }),
                    web_urls: Vec::new(),
                    // 无 PlayerCore 的隔离 rig:配置落点空转(宿主行为归 config_host 测试)。
                    apply_config_base: std::sync::Arc::new(|_tree| {}),
                    stats,
                },
            })
        }

        /// 改写 config.lua 并执行一次重载。
        fn rewrite_and_reload(&mut self, lua_src: &str) -> color_eyre::Result<()> {
            std::fs::write(&self.path, lua_src)?;
            reload_once(&self.path, &mut self.runtime, &self.sender, &self.parts);
            Ok(())
        }

        /// 触发一个具名动作,返回结果。
        fn invoke(&self, name: &str) -> color_eyre::Result<mineral_script::ActionOutcome> {
            Ok(self
                .sender
                .invoke_action(name.to_owned(), /*ctx*/ None, /*args*/ Vec::new())
                .blocking_recv()?)
        }
    }

    #[test]
    fn reload_swaps_action_registry_atomically() -> color_eyre::Result<()> {
        use mineral_script::ActionOutcome;
        let mut rig = Rig::boot(
            r#"
            mineral.action("a.old", function() end)
            return {}
            "#,
        )?;
        assert_eq!(rig.invoke("a.old")?, ActionOutcome::Done, "旧 action 就绪");

        rig.rewrite_and_reload(
            r#"
            mineral.action("b.new", function() end)
            return {}
            "#,
        )?;
        assert_eq!(rig.invoke("b.new")?, ActionOutcome::Done, "新 action 生效");
        assert_eq!(
            rig.invoke("a.old")?,
            ActionOutcome::NotFound,
            "旧 action 整体退役(注册表原子换,无残留)"
        );
        Ok(())
    }

    #[test]
    fn reload_failure_keeps_old_vm_running() -> color_eyre::Result<()> {
        use mineral_script::ActionOutcome;
        let mut rig = Rig::boot(
            r#"
            mineral.action("keep.me", function() end)
            return {}
            "#,
        )?;
        // 写坏的 Lua:eval 失败,旧 VM 必须原样存活。
        rig.rewrite_and_reload("this is not lua ((")?;
        assert_eq!(
            rig.invoke("keep.me")?,
            ActionOutcome::Done,
            "重载失败保留旧脚本,不空窗"
        );
        Ok(())
    }

    /// 热重载成功 / 失败各记一条 script_lifecycle(ReloadOk / ReloadFail)进 stats.db。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reload_records_script_lifecycle() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let store = mineral_stats::StatsStore::open(&dir.path().join("stats.db")).await?;
        let params = crate::params_from_config(mineral_config::Config::defaults()?.stats());
        let (recorder, _actor) = crate::StatsRecorder::spawn(store.clone(), params);
        let mut rig = Rig::boot_with_stats("return {}", recorder)?;
        rig.rewrite_and_reload("return {}")?; // 合法 → ReloadOk
        rig.rewrite_and_reload("this is not lua ((")?; // eval 失败 → ReloadFail
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while store.status().await?.events < 2 {
            if std::time::Instant::now() > deadline {
                color_eyre::eyre::bail!("超时:script_lifecycle 未落两条");
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        Ok(())
    }

    #[test]
    fn reload_seeds_props_into_new_vm() -> color_eyre::Result<()> {
        use mineral_script::ActionOutcome;
        // Rig 的快照源固定给 player.volume = 42(见 boot);重载后的新 VM
        // 顶层 get 与 observe 注册回放都必须立即读到它,不等下次真变更。
        let mut rig = Rig::boot("return {}")?;
        rig.rewrite_and_reload(
            r#"
            if mineral.get("player.volume") == 42 then
                mineral.action("get.seeded", function() end)
            end
            mineral.observe("player.volume", function(v)
                if v == 42 then
                    mineral.action("observe.replayed", function() end)
                end
            end)
            return {}
            "#,
        )?;
        assert_eq!(
            rig.invoke("get.seeded")?,
            ActionOutcome::Done,
            "重载后顶层 mineral.get 必须读到播种的属性值"
        );
        assert_eq!(
            rig.invoke("observe.replayed")?,
            ActionOutcome::Done,
            "重载后 observe 注册必须回放播种的属性值"
        );
        Ok(())
    }

    #[test]
    fn reload_upgrades_from_no_script() -> color_eyre::Result<()> {
        use mineral_script::ActionOutcome;
        // 初始无 action(但有合法脚本);重载加 action 后可触发 —— 覆盖
        // "daemon 起动后才写 config.lua" 的升级语义(runtime None → Some 由
        // boot 的 None 分支太绕,这里以空注册表近似)。
        let mut rig = Rig::boot("return {}")?;
        assert_eq!(rig.invoke("late.comer")?, ActionOutcome::NotFound);
        rig.rewrite_and_reload(
            r#"
            mineral.action("late.comer", function() end)
            return {}
            "#,
        )?;
        assert_eq!(rig.invoke("late.comer")?, ActionOutcome::Done);
        Ok(())
    }
}
