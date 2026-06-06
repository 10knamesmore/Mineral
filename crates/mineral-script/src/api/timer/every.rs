//! `mineral.timer.every(ms, fn)`:周期定时器(脚本线程串行,慢回调不重入),
//! 返回 handle。

use mlua::{Lua, Table};

use crate::api::timer::table::install_ctor;
use crate::host::ScriptHost;

/// 把 `every` 挂到 `timer` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `timer`: `mineral.timer` 子表
///   - `host`: 宿主句柄(共享定时器表)
pub(crate) fn install(lua: &Lua, timer: &Table, host: &ScriptHost) -> mlua::Result<()> {
    install_ctor(lua, timer, host, "every", /*repeating*/ true)
}
