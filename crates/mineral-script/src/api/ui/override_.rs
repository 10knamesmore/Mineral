//! `mineral.ui.override(key, value)`:session 级 UI 旋钮覆盖。
//!
//! daemon 零解释:记 opaque 表 + 转发给订阅 client,key→旋钮的类型化映射
//! 在 client 边缘(未知 key client 侧 warn + 丢)。`value = nil` 撤销覆盖,
//! client 回落自己的配置值;不碰 default.lua 单一真相源,daemon 重启即清。

use mlua::{Lua, Table};

use crate::api::value::lua_to_bus;
use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// 把 `override` 挂到 `ui` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `ui`: `mineral.ui` 子表
///   - `host`: 宿主句柄(闭包捕获其命令出口)
pub(crate) fn install(lua: &Lua, ui: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let commands = host.commands.clone();
    ui.set(
        "override",
        lua.create_function(move |_lua, (key, value): (String, mlua::Value)| {
            // nil 收敛成「撤销」:`Some(Nil)` 不上 wire,避免「覆盖成 Nil」
            // 与「撤销」两义。
            let value = match value {
                mlua::Value::Nil => None,
                other => Some(lua_to_bus(&other, /*depth*/ 0)?),
            };
            // 接收端关闭(daemon 停机)时静默丢,脚本不感知。
            let _ = commands.send(ScriptCmd::UiOverride { key, value });
            Ok(())
        })?,
    )
}

#[cfg(test)]
mod tests {
    use mineral_protocol::BusValue;

    use crate::api::test_support::{drain_cmds, vm_with_commands};
    use crate::message::ScriptCmd;

    #[test]
    fn override_sends_cmd_with_value() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"mineral.ui.override("lyrics.fullscreen_line_gap", 2)"#)
            .exec()?;
        assert_eq!(
            drain_cmds(&mut cmd_rx),
            vec![ScriptCmd::UiOverride {
                key: "lyrics.fullscreen_line_gap".to_owned(),
                value: Some(BusValue::Int(2)),
            }]
        );
        Ok(())
    }

    #[test]
    fn override_nil_converges_to_revoke() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"mineral.ui.override("lyrics.fullscreen_line_gap", nil)"#)
            .exec()?;
        assert_eq!(
            drain_cmds(&mut cmd_rx),
            vec![ScriptCmd::UiOverride {
                key: "lyrics.fullscreen_line_gap".to_owned(),
                value: None,
            }],
            "nil 必须收敛成撤销(None),不得发 Some(Nil)"
        );
        Ok(())
    }

    #[test]
    fn override_rejects_function_value() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        let result = lua
            .load(r#"mineral.ui.override("k", function() end)"#)
            .exec();
        assert!(result.is_err(), "function 值必须报 Lua 错");
        assert!(drain_cmds(&mut cmd_rx).is_empty(), "报错时不得发命令");
        Ok(())
    }
}
