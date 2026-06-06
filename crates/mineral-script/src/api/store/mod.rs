//! `mineral.store.*`:per-song 持久 KV 的脚本出口。
//! 一个文件对应一个 Lua 函数,与脚本侧 API 树一一对应。
//! (端到端行为测试在 `runtime.rs`:真脚本线程 + 模拟 daemon 泵回投。)

pub(crate) mod get;
pub(crate) mod inc;
pub(crate) mod set;

use mlua::{Lua, Table};

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
