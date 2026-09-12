//! 组装脚本日志的 Lua API 表。

use mlua::{Lua, Table};

use super::{info, warn};

/// 组装 `log` 子表并挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
pub(crate) fn install(lua: &Lua, mineral: &Table) -> mlua::Result<()> {
    let log = lua.create_table()?;
    info::install(lua, &log)?;
    warn::install(lua, &log)?;
    mineral.set("log", log)
}
