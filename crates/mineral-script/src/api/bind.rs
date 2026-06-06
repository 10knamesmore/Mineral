//! `mineral.bind(key, fn)`:匿名动作 + 键位绑定的语法糖。
//!
//! 等价于 `mineral.action(内部名, fn)` + 把「key → 内部名」记进 bind 表;
//! client(TUI)经 `Request::ScriptBinds` 拉表,把键合进自己的 keymap。
//! 键字符串文法与 `tui.keys` 一致(nvim 表示法:`"X"` / `"<C-g>"`),**解析在 client 侧**
//! ——daemon 不感知键盘,这里只存字符串。

use std::sync::Arc;

use mineral_protocol::ScriptBind;
use mlua::{Lua, Table};

use crate::host::ScriptHost;

/// 把 `bind` 挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄(闭包捕获其注册表)
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let events = Arc::clone(&host.events);
    mineral.set(
        "bind",
        lua.create_function(move |lua, (key, func): (String, mlua::Function)| {
            if key.is_empty() {
                return Err(mlua::Error::RuntimeError(
                    "bind key must be non-empty".to_owned(),
                ));
            }
            let registry_key = Arc::new(lua.create_registry_value(func)?);
            let mut registry = events.lock();
            let action = registry.next_bind_name();
            registry.actions.insert(action.clone(), registry_key);
            registry.binds.push(ScriptBind { key, action });
            Ok(())
        })?,
    )
}

#[cfg(test)]
mod tests {
    use mineral_protocol::ScriptBind;

    use crate::api::test_support::vm_with_host;

    #[test]
    fn bind_registers_anonymous_action_and_records_key() -> color_eyre::Result<()> {
        let (lua, host) = vm_with_host()?;
        lua.load(
            r#"
            mineral.bind("X", function() end)
            mineral.bind("<C-g>", function() end)
            "#,
        )
        .exec()?;
        let registry = host.events.lock();
        assert_eq!(
            registry.binds,
            vec![
                ScriptBind {
                    key: "X".to_owned(),
                    action: "bind#1".to_owned(),
                },
                ScriptBind {
                    key: "<C-g>".to_owned(),
                    action: "bind#2".to_owned(),
                },
            ],
            "bind 表按注册顺序记录 key → 内部名"
        );
        assert!(
            registry.actions.contains_key("bind#1") && registry.actions.contains_key("bind#2"),
            "匿名 fn 必须以内部名进动作注册表(触发链复用 action)"
        );
        Ok(())
    }

    #[test]
    fn bind_empty_key_is_lua_error() -> color_eyre::Result<()> {
        let (lua, host) = vm_with_host()?;
        assert!(
            lua.load(r#"mineral.bind("", function() end)"#)
                .exec()
                .is_err(),
            "空键必须报 Lua 错"
        );
        assert!(host.events.lock().binds.is_empty());
        Ok(())
    }
}
