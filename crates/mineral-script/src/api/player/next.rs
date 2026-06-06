//! `mineral.player.next()`:下一首。

use mlua::{Lua, Table};

use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `next` 挂到 `player` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `player`: `mineral.player` 子表
///   - `host`: 宿主句柄(闭包捕获其命令出口)
pub(crate) fn install(lua: &Lua, player: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let commands = host.commands.clone();
    player.set(
        "next",
        lua.create_function(move |_lua, ()| {
            let _ = commands.send(ScriptCmd::Next);
            Ok(())
        })?,
    )
}
