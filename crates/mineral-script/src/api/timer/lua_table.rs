//! 组装一次性与周期定时器的 Lua API 表。

use mlua::{Lua, Table};

use super::{after, every};
use crate::host::ScriptHost;

/// 组装 `timer` 子表并挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄(共享定时器表)
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let timer = lua.create_table()?;
    after::install(lua, &timer, host)?;
    every::install(lua, &timer, host)?;
    mineral.set("timer", timer)
}
