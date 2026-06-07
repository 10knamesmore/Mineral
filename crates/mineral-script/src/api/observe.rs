//! `mineral.observe(prop, fn)`:属性树订阅。
//!
//! 「订阅即回放」:注册时属性缓存里已有值就立即调一次回调(在 `observe`
//! 的调用栈内,错误自然冒泡成本次调用的 Lua 错);此后每次
//! `PropertyChanged` 投递再调。缓存由 dispatch 层在投递时更新。

use std::sync::Arc;

use mlua::{Lua, Table};

use crate::api::value::{parse_prop, prop_to_lua};
use crate::host::ScriptHost;

/// 把 `observe` 挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄(闭包捕获其注册表 / 属性缓存)
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let events = Arc::clone(&host.events);
    mineral.set(
        "observe",
        lua.create_function(move |lua, (prop, func): (String, mlua::Function)| {
            let key = parse_prop(&prop)?;
            let replay = {
                let mut registry = events.lock();
                registry
                    .observers
                    .entry(key)
                    .or_default()
                    .push(Arc::new(lua.create_registry_value(func.clone())?));
                registry.props.get(&key).cloned()
            };
            // 锁外回放:回调里再 observe/on 不会撞不可重入锁。
            if let Some(value) = replay {
                func.call::<()>(prop_to_lua(lua, &value)?)?;
            }
            Ok(())
        })?,
    )
}

#[cfg(test)]
mod tests {
    use crate::api::test_support::vm_with_host;
    use crate::message::{PropKey, PropValue};

    #[test]
    fn observe_replays_cached_value_on_registration() -> color_eyre::Result<()> {
        let (lua, host) = vm_with_host()?;
        host.events
            .lock()
            .props
            .insert(PropKey::PlayerMode, PropValue::Str("shuffle".to_owned()));
        lua.load(
            r#"
            seen = nil
            mineral.observe("player.mode", function(v) seen = v end)
            assert(seen == "shuffle", "注册时必须立即回放缓存值")
            "#,
        )
        .exec()?;
        Ok(())
    }

    #[test]
    fn observe_without_cached_value_does_not_fire() -> color_eyre::Result<()> {
        let (lua, host) = vm_with_host()?;
        lua.load(
            r#"
            fired = false
            mineral.observe("queue.length", function() fired = true end)
            assert(fired == false, "无缓存值不得回放")
            "#,
        )
        .exec()?;
        let registry = host.events.lock();
        let observers = registry.observers.get(&PropKey::QueueLength);
        assert_eq!(observers.map(Vec::len), Some(1), "回调必须已注册");
        Ok(())
    }

    #[test]
    fn observe_terminal_replays_table_value() -> color_eyre::Result<()> {
        let (lua, host) = vm_with_host()?;
        host.events.lock().props.insert(
            PropKey::Terminal,
            PropValue::Table(vec![
                ("rows".to_owned(), PropValue::Int(50)),
                ("cols".to_owned(), PropValue::Int(220)),
                ("fullscreen".to_owned(), PropValue::Bool(true)),
            ]),
        );
        lua.load(
            r#"
            mineral.observe("terminal", function(t)
                assert(t.rows == 50, "rows 必须是 50")
                assert(t.cols == 220, "cols 必须是 220")
                assert(t.fullscreen == true, "fullscreen 必须是 true")
            end)
            "#,
        )
        .exec()?;
        Ok(())
    }

    #[test]
    fn unknown_property_is_lua_error() -> color_eyre::Result<()> {
        let (lua, _host) = vm_with_host()?;
        assert!(
            lua.load(r#"mineral.observe("player.lyrics", function() end)"#)
                .exec()
                .is_err(),
            "未知属性名必须报 Lua 错"
        );
        Ok(())
    }

    #[test]
    fn meta_stub_prop_alias_and_overloads_match_rust() -> color_eyre::Result<()> {
        use color_eyre::eyre::WrapErr;
        // `mineral.PropName` 枚举与每个属性的 observe/get @overload 分派行
        // 都必须与 Rust `PropKey::ALL` 同步 —— 缺一行,该属性在编辑器里
        // 就退化回弱类型。
        let meta_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../mineral-config/src/lua/meta/mineral.lua"
        );
        let meta = std::fs::read_to_string(meta_path).wrap_err("read meta/mineral.lua")?;
        let literals = PropKey::ALL
            .map(|key| format!("\"{}\"", key.as_str()))
            .join("|");
        let alias = format!("---@alias mineral.PropName {literals}");
        assert!(
            meta.contains(&alias),
            "meta stub 缺少与 Rust 一致的别名行:`{alias}`"
        );
        for key in PropKey::ALL {
            let observe_overload = format!("---@overload fun(prop: \"{}\", fn:", key.as_str());
            assert!(
                meta.contains(&observe_overload),
                "meta stub 缺少属性 {:?} 的 observe @overload 分派行",
                key.as_str()
            );
            let get_overload = format!("---@overload fun(prop: \"{}\"):", key.as_str());
            assert!(
                meta.contains(&get_overload),
                "meta stub 缺少属性 {:?} 的 get @overload 分派行",
                key.as_str()
            );
        }
        Ok(())
    }
}
