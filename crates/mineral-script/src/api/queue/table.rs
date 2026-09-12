//! 组装队列读取与重排的 Lua API 表。

use mlua::{Lua, Table};

use super::{list, set};
use crate::host::ScriptHost;

/// 组装 `queue` 子表并挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let queue = lua.create_table()?;
    list::install(lua, &queue, host)?;
    set::install(lua, &queue, host)?;
    mineral.set("queue", queue)
}
