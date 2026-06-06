//! `mineral.player.toggle()`:播放 / 暂停切换。

use mlua::{Lua, Table};

use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `toggle` 挂到 `player` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `player`: `mineral.player` 子表
///   - `host`: 宿主句柄(闭包捕获其命令出口)
pub(crate) fn install(lua: &Lua, player: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let commands = host.commands.clone();
    player.set(
        "toggle",
        lua.create_function(move |_lua, ()| {
            let _ = commands.send(ScriptCmd::Toggle);
            Ok(())
        })?,
    )
}
