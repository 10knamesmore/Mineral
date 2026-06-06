//! `mineral.action(name, fn)`:具名动作注册。
//!
//! 触发面:TUI `tui.keys.script` 绑键 / CLI `mineral action <name>`,
//! daemon 转投脚本线程按名查表执行,回调收 ctx table。

use std::sync::Arc;

use mlua::{Lua, Table};

use crate::host::ScriptHost;

/// 把 `action` 挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄(闭包捕获其动作注册表)
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let events = Arc::clone(&host.events);
    mineral.set(
        "action",
        lua.create_function(move |lua, (name, func): (String, mlua::Function)| {
            if name.is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "action name must be non-empty".to_owned(),
                ));
            }
            let key = Arc::new(lua.create_registry_value(func)?);
            let mut registry = events.lock();
            if registry.actions.contains_key(&name) {
                return Err(mlua::Error::RuntimeError(format!(
                    "action {name:?} already registered"
                )));
            }
            registry.actions.insert(name, key);
            Ok(())
        })?,
    )
}

#[cfg(test)]
mod tests {
    use crate::api::test_support::vm_with_host;

    #[test]
    fn action_registers_and_duplicate_is_lua_error() -> color_eyre::Result<()> {
        let (lua, host) = vm_with_host()?;
        lua.load(r#"mineral.action("my.skip_short", function() end)"#)
            .exec()?;
        {
            let registry = host.events.lock();
            assert!(registry.actions.contains_key("my.skip_short"));
        }
        assert!(
            lua.load(r#"mineral.action("my.skip_short", function() end)"#)
                .exec()
                .is_err(),
            "重名注册必须报 Lua 错"
        );
        assert!(
            lua.load(r#"mineral.action("", function() end)"#)
                .exec()
                .is_err(),
            "空名必须报 Lua 错"
        );
        Ok(())
    }
}
