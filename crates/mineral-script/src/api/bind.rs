//! `mineral.bind(key, fn)`:匿名动作 + 键位追加的语法糖。**尚未实现**。
//!
//! 推迟原因:键位追加要动 keys 配置切片与 client 下发链(热重载同通道),
//! 归 Phase 2 的热重载一并落。这里挂 warn + no-op —— 报错会让整个脚本
//! eval 失败弃 VM,过狠。

use mlua::{Lua, Table};

use crate::host::ScriptHost;

/// 把 `bind`(占位)挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄(占位实现不消费,签名与同族一致)
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let _ = host;
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
    use crate::api::test_support::vm_with_host;

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
