//! `mineral.on_message(name, fn)`:订阅自定义总线消息(按名分桶)。

use std::sync::Arc;

use mlua::{Lua, Table};

use crate::host::ScriptHost;

/// 把 `on_message` 挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄(订阅表)
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let h = host.clone();
    mineral.set(
        "on_message",
        lua.create_function(move |lua, (name, callback): (String, mlua::Function)| {
            let key = Arc::new(lua.create_registry_value(callback)?);
            h.events.lock().bus_subs.entry(name).or_default().push(key);
            Ok(())
        })?,
    )
}
