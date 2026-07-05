//! `mineral.hook(name, fn)`:注册同步拦截 hook(`before_stream` / `before_download`),
//! 以及 `mineral.DEFER` 哨兵(回调返回它 = 裁决稍后经 `ctx.resolve(...)` 补交)。
//!
//! 回调按注册顺序调用,首个非放行返回值(或 DEFER)短路生效;返回值约定与
//! 裁决收敛见 dispatch 层 `run_hooks`。

use std::sync::Arc;

use mlua::{Lua, Table};

use crate::hooks::HookKind;
use crate::host::ScriptHost;

/// `mineral.DEFER` 哨兵在 Lua 注册表里的命名键(dispatch 层按指针比对识别)。
pub(crate) const DEFER_REGISTRY_KEY: &str = "mineral.hook.defer";

/// 把 `hook` 与 `DEFER` 挂到 `mineral` 表上。
///
/// `DEFER` 是一张空的唯一 table:身份即指针,值相等的其它空 table 不算——
/// 脚本只能经 `mineral.DEFER` 表达延迟,不会误触。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄(注册表)
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let defer = lua.create_table()?;
    lua.set_named_registry_value(DEFER_REGISTRY_KEY, &defer)?;
    mineral.set("DEFER", defer)?;

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
