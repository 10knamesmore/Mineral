//! daemon Lua API 单测共用的 VM、通道与看门狗构造器。

use mlua::Lua;
use tokio::sync::mpsc::unbounded_channel;

use crate::host::{ScriptHost, install_daemon_api};
use crate::message::ScriptCmd;

/// 装好 daemon 模块的 VM 与宿主，不创建全局 mineral。
pub(crate) fn vm_with_host() -> color_eyre::Result<(Lua, ScriptHost)> {
    let (cmd_tx, _cmd_rx) = unbounded_channel();
    let (push_tx, _push_rx) = unbounded_channel();
    let host = ScriptHost::new(cmd_tx, push_tx);
    let lua = Lua::new();
    install_daemon_api(&lua, &host)?;
    Ok((lua, host))
}

/// 装好 daemon 模块的 VM 与命令接收端。
pub(crate) fn vm_with_commands()
-> color_eyre::Result<(Lua, tokio::sync::mpsc::UnboundedReceiver<ScriptCmd>)> {
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (push_tx, _push_rx) = unbounded_channel();
    let host = ScriptHost::new(cmd_tx, push_tx);
    let lua = Lua::new();
    install_daemon_api(&lua, &host)?;
    host.commands.activate();
    Ok((lua, cmd_rx))
}

/// 排干音乐命令通道。
pub(crate) fn drain_cmds(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<ScriptCmd>,
) -> Vec<ScriptCmd> {
    let mut cmds = Vec::new();
    while let Ok(cmd) = rx.try_recv() {
        cmds.push(cmd);
    }
    cmds
}
