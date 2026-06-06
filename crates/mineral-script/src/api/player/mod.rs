//! `mineral.player.*`:播放器控制命令族(全部 fire-and-forget)。
//! 一个文件对应一个 Lua 函数,与脚本侧 API 树一一对应。

pub(crate) mod next;
pub(crate) mod play;
pub(crate) mod prev;
pub(crate) mod seek_rel;
pub(crate) mod seek_to;
pub(crate) mod set_mode;
pub(crate) mod set_volume;
pub(crate) mod stop;
pub(crate) mod toggle;

#[cfg(test)]
mod tests;

use mlua::{Lua, Table};

use crate::host::ScriptHost;

/// 组装 `player` 子表并挂到 `mineral` 表上(逐函数分发给各文件)。
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
