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
    let (_config, warnings, vm) = match loaded {
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
            return;
        }
    };
    let Some(lua) = vm else {
        // eval 失败 / 文件缺失:保留旧 VM 照常跑,报错给用户。
        let detail = warnings.first().map_or_else(
            || "config.lua 缺失或未通过求值".to_owned(),
            std::string::ToString::to_string,
        );
        mineral_log::warn!(target: "script", detail, "脚本重载失败,保留旧脚本");
        toast(
            &parts.push_tx,
            ToastKind::Error,
            format!("脚本重载失败,保留旧脚本:{detail}"),
        );
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
            // 先发就绪信号再发 toast:client 收到 ScriptReloaded 重拉 binds 时表已就绪。
            let _ = parts.push_tx.send(Event::ScriptReloaded);
            toast(&parts.push_tx, ToastKind::Info, "脚本已热重载".to_owned());
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
        }
    }
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
            let dir = tempfile::tempdir()?;
            let path = dir.path().join("config.lua");
            std::fs::write(&path, initial_lua)?;
            let (cmd_tx, _cmd_rx) = unbounded_channel();
            let (push_tx, _push_rx) = unbounded_channel();
            let host = ScriptHost::new(cmd_tx.clone(), push_tx.clone());
            let (_cfg, warnings, vm) = mineral_config::load_with_vm(&path, |lua| {
                install_api(lua, &host).map_err(color_eyre::Report::new)
            })?;
            assert!(warnings.is_empty(), "初始配置不应有 warning: {warnings:?}");
            let sender = ScriptSender::detached();
            let lua = vm.ok_or_else(|| color_eyre::eyre::eyre!("初始 VM 应就绪"))?;
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
