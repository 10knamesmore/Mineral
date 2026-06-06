//! `mineral.log.*`:脚本写日志。
//! 一个文件对应一个 Lua 函数,与脚本侧 API 树一一对应。

pub(crate) mod info;
pub(crate) mod warn;

#[cfg(test)]
mod tests;

use mlua::{Lua, Table};

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
