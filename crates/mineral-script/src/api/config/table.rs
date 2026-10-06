//! 组装配置覆盖的 Lua API 表。

use mlua::{Lua, Table};

use super::override_;
use crate::message::ConfigOverrideOp;

/// 组装 `config` 子表并挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 宿主模块表
///   - `send`: 接收已解析覆盖操作的宿主出口
pub(crate) fn install(
    lua: &Lua,
    mineral: &Table,
    send: impl Fn(Vec<ConfigOverrideOp>) + Send + 'static,
) -> mlua::Result<()> {
    let config = lua.create_table()?;
    override_::install(lua, &config, send)?;
    mineral.set("config", config)
}
