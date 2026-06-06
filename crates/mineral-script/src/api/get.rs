//! `mineral.get(prop)`:读属性树当前值。
//!
//! daemon 尚未推送过该属性时返回 nil(与「无在播歌」的 None 在 Lua 侧同形)。

use std::sync::Arc;

use mlua::{Lua, Table};

use crate::api::value::{parse_prop, prop_to_lua};
use crate::host::ScriptHost;

/// 把 `get` 挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄(闭包捕获其属性缓存)
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let events = Arc::clone(&host.events);
    mineral.set(
        "get",
        lua.create_function(move |lua, prop: String| {
            let key = parse_prop(&prop)?;
            let value = events.lock().props.get(&key).cloned();
            match value {
                Some(value) => prop_to_lua(lua, &value),
                // 尚未推送过:nil。
                None => Ok(mlua::Value::Nil),
            }
        })?,
    )
}

#[cfg(test)]
mod tests {
    use crate::api::test_support::vm_with_host;
    use crate::message::{PropKey, PropValue};

    #[test]
    fn get_returns_nil_before_any_push_then_cached_value() -> color_eyre::Result<()> {
        let (lua, host) = vm_with_host()?;
        lua.load(r#"assert(mineral.get("player.volume") == nil)"#)
            .exec()?;
        host.events
            .lock()
            .props
            .insert(PropKey::PlayerVolume, PropValue::Int(42));
        lua.load(r#"assert(mineral.get("player.volume") == 42)"#)
            .exec()?;
        Ok(())
    }

    #[test]
    fn get_validates_property_name() -> color_eyre::Result<()> {
        let (lua, _host) = vm_with_host()?;
        assert!(
            lua.load(r#"mineral.get("player.lyrics")"#).exec().is_err(),
            "未知属性名必须报 Lua 错"
        );
        Ok(())
    }
}
