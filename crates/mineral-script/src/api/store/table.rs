//! 组装歌曲持久 KV 的 Lua API 表。

use mlua::{Lua, Table};

use super::{get, inc, set};
use crate::host::ScriptHost;

/// 组装 `store` 子表并挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let store = lua.create_table()?;
    get::install(lua, &store, host)?;
    set::install(lua, &store, host)?;
    inc::install(lua, &store, host)?;
    mineral.set("store", store)
}
