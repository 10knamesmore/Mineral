//! `mineral.config.*`:脚本对有效配置的 session 级覆盖出口。
//! 一个文件对应一个 Lua 函数,与脚本侧 API 树一一对应。

pub(crate) mod override_;

use mlua::{Lua, Table};

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
