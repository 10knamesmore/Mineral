//! `mineral.player.set_volume(pct)`:设音量(越界 clamp 到 0..=100,不报错)。

use mlua::{Lua, Table};

use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `set_volume` 挂到 `player` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `player`: `mineral.player` 子表
///   - `host`: 宿主句柄(闭包捕获其命令出口)
pub(crate) fn install(lua: &Lua, player: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let commands = host.commands.clone();
    player.set(
        "set_volume",
        lua.create_function(move |_lua, pct: i64| {
            // 越界 clamp 到 0..=100(用户裁决:容忍,不报错)。
            let clamped = u8::try_from(pct.clamp(0, 100)).unwrap_or(100);
            let _ = commands.send(ScriptCmd::SetVolume(clamped));
            Ok(())
        })?,
    )
}
