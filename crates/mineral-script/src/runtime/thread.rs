//! daemon 脚本线程的生命周期：移交 VM，Drop 停机并 join。

use mlua::Lua;

use crate::dispatch;
use crate::host::ScriptHost;
use crate::message::ScriptMsg;
use crate::sender::ScriptSender;
use crate::watchdog::WatchdogConfig;
use crate::{Error, Result};

/// daemon 脚本线程句柄。Drop 等待在跑的回调结束，由看门狗约束 Lua 执行。
#[derive(Debug)]
pub struct ScriptRuntime {
    /// 消息入口，克隆给 [`ScriptSender`]。
    tx: std::sync::mpsc::Sender<ScriptMsg>,

    /// Drop 时取出并 join 的线程句柄。
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ScriptRuntime {
    /// 在看门狗下执行 daemon.lua 的 setup(host)，成功后将 VM 移交专用线程。
    ///
    /// # Params:
    ///   - `lua`: 配置加载已成功、持有 setup 与音乐回调的 daemon VM
    ///   - `host`: 与 Lua API 闭包共享的注册表和通道
    ///   - `watchdog`: 回调看门狗参数
    ///   - `sender`: 启动成功后原子挂接的 daemon 投递句柄
    ///
    /// # Return:
    ///   线程句柄；setup 或线程创建失败时不提交暂存命令，`sender` 保持原挂接。
    pub fn spawn(
        lua: Lua,
        host: ScriptHost,
        watchdog: WatchdogConfig,
        sender: &ScriptSender,
    ) -> Result<Self> {
        crate::setup::run(
            &lua,
            &watchdog,
            crate::registry::DAEMON_SETUP_FN,
            "mineral.daemon",
        )
        .map_err(|source| Error::Lua {
            operation: "执行 daemon.lua 的 setup",
            source,
        })?;
        let (tx, rx) = std::sync::mpsc::channel();
        let commands = host.commands.clone();
        let handle = std::thread::Builder::new()
            .name("mineral-script".to_owned())
            .spawn(move || dispatch::run_loop(&lua, &host, &watchdog, &rx))
            .map_err(Error::Thread)?;
        sender.attach(tx.clone());
        commands.activate();
        mineral_log::debug!(target: "script", "daemon script runtime activated");
        Ok(Self {
            tx,
            handle: Some(handle),
        })
    }
}

impl Drop for ScriptRuntime {
    fn drop(&mut self) {
        let _ = self.tx.send(ScriptMsg::Stop);
        if let Some(handle) = self.handle.take()
            && handle.join().is_err()
        {
            mineral_log::error!(target: "script", "mineral-script thread panicked");
        }
    }
}
