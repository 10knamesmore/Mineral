//! 将当前宿主的 API 注册为 Lua require 模块。

/// 注册 require 可读取的模块，不向 Lua 全局表写宿主 API。
pub(crate) fn install_module(lua: &mlua::Lua, name: &str, module: mlua::Table) -> mlua::Result<()> {
    let package = lua.globals().get::<mlua::Table>("package")?;
    let loaded = package.get::<mlua::Table>("loaded")?;
    loaded.set(name, module)
}

/// 读取已经安装的宿主表，作为 setup 参数；不执行用户定义的 require loader。
pub(crate) fn module_table(lua: &mlua::Lua, name: &str) -> mlua::Result<mlua::Table> {
    let package = lua.globals().get::<mlua::Table>("package")?;
    let loaded = package.get::<mlua::Table>("loaded")?;
    loaded.get(name)
}
