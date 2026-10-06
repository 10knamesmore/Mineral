//! 组装界面提示与标题覆盖的 Lua API 表。

use mlua::{Lua, Table};

use super::{card, toast, window_title};
use crate::tui::TuiHost;

/// 组装 `ui` 子表并挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: `mineral.tui` 模块表
///   - `host`: 本地配置、setup 与复制回调的效果收集器
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &TuiHost) -> mlua::Result<()> {
    let ui = lua.create_table()?;
    toast::install(lua, &ui, host)?;
    card::install(lua, &ui, host)?;
    window_title::install(lua, &ui, host)?;
    mineral.set("ui", ui)
}
