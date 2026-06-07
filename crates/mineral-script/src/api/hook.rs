//! `mineral.hook(name, fn)`:注册同步拦截 hook(`before_play` / `before_download`)。
//!
//! 回调按注册顺序调用,首个非放行返回值短路生效;返回值约定与裁决收敛
//! 见 dispatch 层 `run_hooks`。

use std::sync::Arc;

use mlua::{Lua, Table};

use crate::hooks::HookKind;
use crate::host::ScriptHost;

/// 把 `hook` 挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄(注册表)
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let h = host.clone();
    mineral.set(
        "hook",
        lua.create_function(move |lua, (name, callback): (String, mlua::Function)| {
            let Some(kind) = HookKind::from_name(&name) else {
                let known = HookKind::ALL.map(HookKind::as_str).join(" / ");
                return Err(mlua::Error::runtime(format!(
                    "未知 hook `{name}`(可选:{known})"
                )));
            };
            let key = Arc::new(lua.create_registry_value(callback)?);
            h.events.lock().hooks.entry(kind).or_default().push(key);
            Ok(())
        })?,
    )
}
