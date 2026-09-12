//! 组装界面提示与标题覆盖的 Lua API 表。

use mlua::{Lua, Table};

use super::{card, toast, window_title};
use crate::host::ScriptHost;

/// 组装 `ui` 子表并挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let ui = lua.create_table()?;
    toast::install(lua, &ui, host)?;
    card::install(lua, &ui, host)?;
    window_title::install(lua, &ui, host)?;
    mineral.set("ui", ui)
}
