//! `mineral.player.set_mode(mode)`:设播放模式(蛇形稳定名,未知名报错)。

use mineral_protocol::PlayMode;
use mlua::{Lua, Table};

use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `set_mode` 挂到 `player` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `player`: `mineral.player` 子表
///   - `host`: 宿主句柄(闭包捕获其命令出口)
pub(crate) fn install(lua: &Lua, player: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let commands = host.commands.clone();
    player.set(
        "set_mode",
        lua.create_function(move |_lua, mode: String| {
            let Some(mode) = PlayMode::from_script_name(&mode) else {
                return Err(mlua::Error::RuntimeError(format!(
                    "unknown play mode {mode:?}, expected \"sequential\" | \"shuffle\" | \"repeat_all\" | \"repeat_one\""
                )));
            };
            let _ = commands.send(ScriptCmd::SetMode(mode));
            Ok(())
        })?,
    )
}
