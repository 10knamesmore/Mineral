//! 组装配置覆盖的 Lua API 表。

use mlua::{Lua, Table};

use super::override_;
use crate::host::ScriptHost;

/// 组装 `config` 子表并挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let config = lua.create_table()?;
    override_::install(lua, &config, host)?;
    mineral.set("config", config)
}
