//! api 各模块单测的共享构造器(仅 `#[cfg(test)]` 编译)。

use mlua::Lua;
use tokio::sync::mpsc::unbounded_channel;

use crate::host::{ScriptHost, install_api};
use crate::message::ScriptCmd;

/// 装好 API 的 VM + 宿主句柄(注册表断言用)。
pub(crate) fn vm_with_host() -> color_eyre::Result<(Lua, ScriptHost)> {
    let (cmd_tx, _cmd_rx) = unbounded_channel();
    let (push_tx, _push_rx) = unbounded_channel();
    let host = ScriptHost::new(cmd_tx, push_tx);
    let lua = Lua::new();
    install_api(&lua, &host)?;
    Ok((lua, host))
}

/// 装好 API 的 VM + 命令接收端(命令形状断言用)。
pub(crate) fn vm_with_commands()
-> color_eyre::Result<(Lua, tokio::sync::mpsc::UnboundedReceiver<ScriptCmd>)> {
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (push_tx, _push_rx) = unbounded_channel();
    let host = ScriptHost::new(cmd_tx, push_tx);
    let lua = Lua::new();
    install_api(&lua, &host)?;
    Ok((lua, cmd_rx))
}

/// 装好 API 的 VM + 推送接收端(toast / 推送形状断言用)。
pub(crate) fn vm_with_push() -> color_eyre::Result<(
    Lua,
    tokio::sync::mpsc::UnboundedReceiver<mineral_protocol::Event>,
)> {
    let (cmd_tx, _cmd_rx) = unbounded_channel();
    let (push_tx, push_rx) = unbounded_channel();
    let host = ScriptHost::new(cmd_tx, push_tx);
    let lua = Lua::new();
    install_api(&lua, &host)?;
    Ok((lua, push_rx))
}

/// 排干命令通道。
pub(crate) fn drain_cmds(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<ScriptCmd>,
) -> Vec<ScriptCmd> {
    let mut cmds = Vec::new();
    while let Ok(cmd) = rx.try_recv() {
        cmds.push(cmd);
    }
    cmds
}
