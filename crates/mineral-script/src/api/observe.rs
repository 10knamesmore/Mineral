//! `mineral.observe(prop, fn)` 与 `mineral.get(prop)`:属性树的订阅与读取。
//!
//! 「订阅即回放」:注册时属性缓存里已有值就立即调一次回调(在 `observe`
//! 的调用栈内,错误自然冒泡成本次调用的 Lua 错);此后每次
//! `PropertyChanged` 投递再调。缓存由 dispatch 层在投递时更新,daemon
//! 尚未推送过的属性 `get` 返回 nil、observe 不回放。

use std::sync::Arc;

use mlua::{Lua, Table};

use crate::api::value::prop_to_lua;
use crate::host::ScriptHost;
use crate::message::PropKey;

/// 把 `observe` / `get` 挂到 `mineral` 表上。
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
    )?;

    let events = Arc::clone(&host.events);
    mineral.set(
        "get",
        lua.create_function(move |lua, prop: String| {
            let key = parse_prop(&prop)?;
            let value = events.lock().props.get(&key).cloned();
            match value {
                Some(value) => prop_to_lua(lua, &value),
                // 尚未推送过:nil(与「无在播歌」的 None 在 Lua 侧同形)。
                None => Ok(mlua::Value::Nil),
            }
        })?,
    )
}

/// 解析属性名;未知名报 Lua 错并列出全部合法名。
fn parse_prop(prop: &str) -> mlua::Result<PropKey> {
    PropKey::from_name(prop).ok_or_else(|| {
        let expected = PropKey::ALL.map(PropKey::as_str).join("\" | \"");
        mlua::Error::RuntimeError(format!(
            "unknown property {prop:?}, expected \"{expected}\""
        ))
    })
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc::unbounded_channel;

    use crate::host::{ScriptHost, install_api};
    use crate::message::{PropKey, PropValue};

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

    #[test]
    fn unknown_property_is_lua_error() -> color_eyre::Result<()> {
        let (lua, _host) = vm_with_host()?;
        assert!(
            lua.load(r#"mineral.observe("player.lyrics", function() end)"#)
                .exec()
                .is_err(),
            "未知属性名必须报 Lua 错"
        );
        assert!(
            lua.load(r#"mineral.get("player.lyrics")"#).exec().is_err(),
            "get 同样校验属性名"
        );
        Ok(())
    }
}
