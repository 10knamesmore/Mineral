//! `mineral.ui.*`:脚本对用户界面的提示出口。
//! 一个文件对应一个 Lua 函数,与脚本侧 API 树一一对应。

pub(crate) mod toast;

use mlua::{Lua, Table};

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
    mineral.set("ui", ui)
}
