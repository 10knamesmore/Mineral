//! 组装播放器控制的 Lua API 表。

use mlua::{Lua, Table};

use super::{next, play, prev, seek_rel, seek_to, set_mode, set_volume, stop, toggle};
use crate::host::ScriptHost;

/// 组装 `player` 子表并挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let player = lua.create_table()?;
    toggle::install(lua, &player, host)?;
    next::install(lua, &player, host)?;
    prev::install(lua, &player, host)?;
    stop::install(lua, &player, host)?;
    seek_rel::install(lua, &player, host)?;
    seek_to::install(lua, &player, host)?;
    set_volume::install(lua, &player, host)?;
    set_mode::install(lua, &player, host)?;
    play::install(lua, &player, host)?;
    mineral.set("player", player)
}
