//! `mineral.action(name, fn)`:具名动作注册。触发面(client 发起 →
//! daemon 转投脚本线程)是 PR-4 的事,本期只落注册表。
//!
//! `mineral.bind` 推迟(键位追加要动 keys 配置与 client 触发链):
//! 这里挂一个 warn + no-op —— 报错会让整个脚本 eval 失败弃 VM,过狠。

use std::sync::Arc;

use mlua::{Lua, Table};

use crate::host::ScriptHost;

/// 把 `action` / `bind`(占位)挂到 `mineral` 表上。
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
    )?;

    mineral.set(
        "bind",
        lua.create_function(|_lua, (key, _func): (String, mlua::Function)| {
            mineral_log::warn!(target: "script", key, "mineral.bind 尚未实现,本次注册被忽略");
            Ok(())
        })?,
    )
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::unbounded_channel;

    use crate::host::{ScriptHost, install_api};

    /// 装好 API 的 VM + 宿主句柄。
    fn vm_with_host() -> color_eyre::Result<(mlua::Lua, ScriptHost)> {
        let (cmd_tx, _cmd_rx) = unbounded_channel();
        let (push_tx, _push_rx) = unbounded_channel();
        let host = ScriptHost::new(cmd_tx, push_tx);
        let lua = mlua::Lua::new();
        install_api(&lua, &host)?;
        Ok((lua, host))
    }

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

    #[test]
    fn bind_is_noop_placeholder() -> color_eyre::Result<()> {
        let (lua, host) = vm_with_host()?;
        // 不报错、不注册 —— 只留一条 warn 日志。
        lua.load(r#"mineral.bind("<C-s>", function() end)"#)
            .exec()?;
        assert!(host.events.lock().actions.is_empty());
        Ok(())
    }
}
