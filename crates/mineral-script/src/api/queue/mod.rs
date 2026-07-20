//! `mineral.queue.*`:播放队列的脚本出口(读队列 + 整表重排)。
//! 一个文件对应一个 Lua 函数,与脚本侧 API 树一一对应。

pub(crate) mod list;
pub(crate) mod set;

use mlua::{Lua, Table};

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
