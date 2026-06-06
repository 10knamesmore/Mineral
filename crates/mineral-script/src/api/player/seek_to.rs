//! `mineral.player.seek_to(secs)`:绝对 seek(秒;负数压回 0)。

use mlua::{Lua, Table};

use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `seek_to` 挂到 `player` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `player`: `mineral.player` 子表
///   - `host`: 宿主句柄(闭包捕获其命令出口)
pub(crate) fn install(lua: &Lua, player: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let commands = host.commands.clone();
    player.set(
        "seek_to",
        lua.create_function(move |_lua, secs: f64| {
            // 负数压回 0(与音量 clamp 同一容忍风格)。
            let _ = commands.send(ScriptCmd::SeekTo(secs.max(0.0)));
            Ok(())
        })?,
    )
}
