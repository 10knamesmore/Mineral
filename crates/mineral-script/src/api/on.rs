//! `mineral.on(event, fn)`:离散生命周期事件的回调注册。
//!
//! 合法事件名是**封闭集合**([`EVENT_NAMES`]),运行期未知名直接报 Lua 错;
//! 编辑期由 `mineral-config` 分发的 LuaCATS stub(`meta/mineral.lua` 的
//! `mineral.EventName` 字符串枚举别名)约束 —— 两边由守卫测试钉死同步。

use std::sync::Arc;

use mlua::{Lua, Table};

use crate::host::ScriptHost;

/// `mineral.on` 接受的全部事件名。新增事件:加变体进
/// [`EventRegistry`](crate::host::EventRegistry)、这里加名字、
/// meta stub 的 `mineral.EventName` 别名加字面量(守卫测试会逼你同步)。
pub(crate) const EVENT_NAMES: [&str; 2] = ["track_finished", "download_completed"];

/// 把 `on` 挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄(闭包捕获其事件注册表)
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let events = Arc::clone(&host.events);
    mineral.set(
        "on",
        lua.create_function(move |lua, (name, func): (String, mlua::Function)| {
            let key = Arc::new(lua.create_registry_value(func)?);
            let mut registry = events.lock();
            match name.as_str() {
                "track_finished" => registry.track_finished.push(key),
                "download_completed" => registry.download_completed.push(key),
                other => {
                    let expected = EVENT_NAMES.join("\" | \"");
                    return Err(mlua::Error::RuntimeError(format!(
                        "unknown event {other:?}, expected \"{expected}\""
                    )));
                }
            }
            Ok(())
        })?,
    )
}

#[cfg(test)]
mod tests {
    use super::EVENT_NAMES;
    use crate::api::test_support::vm_with_host;

    #[test]
    fn on_registers_callbacks_in_order() -> color_eyre::Result<()> {
        let (lua, host) = vm_with_host()?;
        lua.load(
            r#"
            mineral.on("track_finished", function() end)
            mineral.on("track_finished", function() end)
            mineral.on("download_completed", function() end)
            "#,
        )
        .exec()?;
        let registry = host.events.lock();
        assert_eq!(registry.track_finished.len(), 2);
        assert_eq!(registry.download_completed.len(), 1);
        Ok(())
    }

    #[test]
    fn unknown_event_name_is_lua_error() -> color_eyre::Result<()> {
        let (lua, host) = vm_with_host()?;
        let result = lua
            .load(r#"mineral.on("track_started", function() end)"#)
            .exec();
        assert!(result.is_err(), "未知事件名必须报 Lua 错");
        let registry = host.events.lock();
        assert!(registry.track_finished.is_empty());
        assert!(registry.download_completed.is_empty());
        Ok(())
    }

    #[test]
    fn meta_stub_event_alias_matches_rust_wall() -> color_eyre::Result<()> {
        use color_eyre::eyre::WrapErr;
        // meta/mineral.lua 的 `mineral.EventName` 字符串枚举是编辑期第一道闸,
        // 必须与 Rust 事件墙的合法名集合逐字一致 —— 改一边不改另一边在此爆掉。
        let meta_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../mineral-config/src/lua/meta/mineral.lua"
        );
        let meta = std::fs::read_to_string(meta_path).wrap_err("read meta/mineral.lua")?;
        let literals = EVENT_NAMES.map(|name| format!("\"{name}\"")).join("|");
        let alias = format!("---@alias mineral.EventName {literals}");
        assert!(
            meta.contains(&alias),
            "meta stub 缺少与 Rust 事件墙一致的别名行:`{alias}`"
        );
        assert!(
            meta.contains("---@param event mineral.EventName"),
            "mineral.on 的 event 参数必须引用 mineral.EventName 别名"
        );
        // 每个事件还必须有按字面量分派的 @overload —— 缺了它,该事件的
        // 回调 args 在编辑器里就退化回弱类型。
        for name in EVENT_NAMES {
            let overload = format!("---@overload fun(event: \"{name}\",");
            assert!(
                meta.contains(&overload),
                "meta stub 缺少事件 {name:?} 的 @overload 分派行"
            );
        }
        Ok(())
    }
}
