//! `mineral.emit(name, payload?)`:发一条自定义总线消息。
//!
//! 双路扇出:1)结构化载荷经 push 通道下推订阅 `Bus` 的 client(daemon
//! 零解释转发);2)本 VM 的 `on_message` 订阅者**同步**收到(payload
//! 原引用传递,订阅者间共享同一 table)。单个订阅者出错不影响其余,
//! 也不向 emit 调用方冒泡。

use mlua::{Lua, Table};

use crate::api::value::lua_to_bus;
use crate::host::ScriptHost;

/// 把 `emit` 挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄(订阅表 + 推送出口)
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let h = host.clone();
    mineral.set(
        "emit",
        lua.create_function(move |lua, (name, payload): (String, Option<mlua::Value>)| {
            let payload = payload.unwrap_or(mlua::Value::Nil);
            // 1) 下推 client:VM 边界转结构化载荷(非法载荷当场报错,不发半截)。
            let bus = lua_to_bus(&payload, /*depth*/ 0)?;
            let _ = h.push.send(mineral_protocol::Event::BusMessage {
                name: name.clone(),
                payload: bus,
            });
            // 2) 本 VM 自环:锁内只克隆 Arc 列表,锁外调(订阅者里再 emit 不撞锁)。
            let callbacks = h
                .events
                .lock()
                .bus_subs
                .get(&name)
                .cloned()
                .unwrap_or_default();
            for key in &callbacks {
                let outcome = lua
                    .registry_value::<mlua::Function>(key)
                    .and_then(|func| func.call::<()>(payload.clone()));
                if let Err(e) = outcome {
                    crate::dispatch::report_callback_failure(&h, &name, &e);
                }
            }
            Ok(())
        })?,
    )
}
