//! `mineral.log.info(msg)`:脚本写 info 级日志。

use mlua::{Lua, Table};

/// 把 `info` 挂到 `log` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `log`: `mineral.log` 子表
pub(crate) fn install(lua: &Lua, log: &Table) -> mlua::Result<()> {
    log.set(
        "info",
        lua.create_function(|_lua, msg: String| {
            mineral_log::info!(target: "script", "{msg}");
            Ok(())
        })?,
    )
}
